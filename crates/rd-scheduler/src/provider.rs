//! Provider-wide concurrency and persistent resolver-version routing.

use std::sync::Arc;

use anyhow::Result;
use rd_core::{AccountId, DownloadId};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;

use crate::SchedulerHandle;

pub(crate) enum ProviderSlot {
    Unrestricted,
    Acquired(OwnedSemaphorePermit),
    Busy,
}

/// One plugin's premium concurrency gate, together with the limit it is currently sized for.
///
/// The limit has to be carried alongside: a semaphore only reports the permits that are free
/// right now, which says nothing about how many it was built with while downloads hold some.
pub(crate) struct ProviderGate {
    limit: u32,
    semaphore: Arc<Semaphore>,
}

impl ProviderGate {
    fn new(limit: u32) -> Self {
        Self {
            limit,
            semaphore: Arc::new(Semaphore::new(limit as usize)),
        }
    }

    /// Resizes the gate to the limit the manifest now states.
    ///
    /// Without this the gate kept whatever `max-concurrent-downloads` was in force when the
    /// plugin's first download ran, so upgrading a plugin to raise or lower it changed
    /// nothing until the service restarted.
    ///
    /// Zero needs no guard: the manifest validator rejects a limit below one, so this can
    /// never close the gate outright and strand a provider.
    fn resize(&mut self, limit: u32) {
        if limit > self.limit {
            self.semaphore.add_permits((limit - self.limit) as usize);
            self.limit = limit;
        } else if limit < self.limit {
            // Only free permits can be taken away; the ones in flight belong to downloads
            // that are already running and must finish at the limit they started under.
            // Whatever could not be forgotten now is forgotten on a later call, as those
            // permits come back, so the gate converges on the new limit instead of
            // cancelling work to reach it immediately.
            let surplus = (self.limit - limit) as usize;
            let forgotten = self.semaphore.forget_permits(surplus);
            self.limit = self
                .limit
                .saturating_sub(u32::try_from(forgotten).unwrap_or(u32::MAX));
        }
    }
}

impl SchedulerHandle {
    pub(crate) async fn try_provider_slot(
        &self,
        download_id: DownloadId,
        account_id: Option<AccountId>,
        source: &Url,
    ) -> Result<ProviderSlot> {
        if account_id.is_none() {
            return Ok(self.try_free_slot(source).await);
        }
        let existing = self.database.resolver_pin(download_id).await?;
        let route = self
            .resolvers
            .concurrency_route(account_id, existing.as_ref())
            .await;
        let Some((suggested, mut limit)) = (match route {
            Ok(route) => route,
            Err(failure) => {
                tracing::warn!(%download_id, %failure, "pinned resolver is unavailable");
                return Ok(ProviderSlot::Unrestricted);
            }
        }) else {
            return Ok(ProviderSlot::Unrestricted);
        };
        let pin = if existing.is_some() {
            suggested
        } else {
            self.database
                .claim_resolver_pin(download_id, suggested)
                .await?
        };
        if existing.is_none() {
            let Some((_, pinned_limit)) = self
                .resolvers
                .concurrency_route(account_id, Some(&pin))
                .await
                .map_err(anyhow::Error::new)?
            else {
                return Ok(ProviderSlot::Unrestricted);
            };
            limit = pinned_limit;
        }
        let semaphore = {
            let mut slots = self.provider_slots.lock().await;
            let gate = slots
                .entry(pin.plugin_id)
                .or_insert_with(|| ProviderGate::new(limit));
            // The manifest is re-read on every attempt, so this is where an upgraded plugin's
            // new limit reaches a gate that already exists.
            gate.resize(limit);
            Arc::clone(&gate.semaphore)
        };
        Ok(match semaphore.try_acquire_owned() {
            Ok(permit) => ProviderSlot::Acquired(permit),
            Err(_) => ProviderSlot::Busy,
        })
    }

    /// Serialises a hoster's free downloads: one at a time, regardless of the plugin's
    /// premium concurrency.
    ///
    /// Two reasons, both hard requirements. Free flows share the anonymous HTTP client and
    /// therefore one cookie jar, so two of them running at once would overwrite each
    /// other's session; and hosters grant exactly one free download per IP anyway, so a
    /// second attempt would only earn an IP block for the first one too.
    async fn try_free_slot(&self, source: &Url) -> ProviderSlot {
        let Some(plugin_id) = self.resolvers.free_resolver_plugin(source) else {
            return ProviderSlot::Unrestricted;
        };
        let semaphore = {
            let mut slots = self.free_slots.lock().await;
            Arc::clone(
                slots
                    .entry(plugin_id)
                    .or_insert_with(|| Arc::new(Semaphore::new(1))),
            )
        };
        match semaphore.try_acquire_owned() {
            Ok(permit) => ProviderSlot::Acquired(permit),
            Err(_) => ProviderSlot::Busy,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{ProviderGate, ProviderSlot};

    /// A plugin upgrade that raises `max-concurrent-downloads` has to take effect on the
    /// gate that already exists; before this the limit was frozen at whatever the plugin's
    /// first download ever saw, and only a restart picked the new one up.
    #[test]
    fn raising_a_plugin_limit_widens_the_gate() {
        let mut gate = ProviderGate::new(2);
        assert_eq!(gate.semaphore.available_permits(), 2);

        gate.resize(5);

        assert_eq!(gate.limit, 5);
        assert_eq!(gate.semaphore.available_permits(), 5);
    }

    #[test]
    fn lowering_a_plugin_limit_narrows_an_idle_gate_at_once() {
        let mut gate = ProviderGate::new(4);

        gate.resize(1);

        assert_eq!(gate.limit, 1);
        assert_eq!(gate.semaphore.available_permits(), 1);
    }

    /// Downloads already running finish at the limit they started under: a permit in flight
    /// cannot be taken back. The gate narrows as those permits return instead, so it reaches
    /// the new limit without cancelling work to get there.
    #[test]
    fn lowering_a_plugin_limit_narrows_as_running_downloads_release_their_permits() {
        let mut gate = ProviderGate::new(4);
        // Owned permits, so nothing borrows the gate across the `resize` calls below.
        let semaphore = Arc::clone(&gate.semaphore);
        let first = Arc::clone(&semaphore)
            .try_acquire_owned()
            .expect("a free slot");
        let second = Arc::clone(&semaphore)
            .try_acquire_owned()
            .expect("a free slot");
        let third = Arc::clone(&semaphore)
            .try_acquire_owned()
            .expect("a free slot");

        gate.resize(1);
        assert_eq!(gate.limit, 3, "only the one free permit could be forgotten");
        assert_eq!(semaphore.available_permits(), 0);

        drop(first);
        gate.resize(1);
        assert_eq!(gate.limit, 2);
        assert_eq!(semaphore.available_permits(), 0);

        drop(second);
        drop(third);
        gate.resize(1);
        assert_eq!(gate.limit, 1, "the gate has caught up with the manifest");
        assert_eq!(semaphore.available_permits(), 1);
    }

    /// The steady state: the manifest is re-read on every attempt, and an unchanged limit
    /// must leave the gate — and anything running through it — exactly as it was.
    #[test]
    fn an_unchanged_limit_leaves_the_gate_alone() {
        let mut gate = ProviderGate::new(3);
        let semaphore = Arc::clone(&gate.semaphore);
        let held = Arc::clone(&semaphore)
            .try_acquire_owned()
            .expect("a free slot");

        gate.resize(3);
        gate.resize(3);

        assert_eq!(gate.limit, 3);
        assert_eq!(semaphore.available_permits(), 2);
        drop(held);
        assert_eq!(semaphore.available_permits(), 3);
    }

    async fn scheduler(directory: &std::path::Path) -> crate::SchedulerHandle {
        let database = rd_db::Database::open(directory.join("scheduler.sqlite3"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
            .await
            .expect("secrets");
        crate::SchedulerHandle::start(
            database,
            crate::SchedulerConfig::for_directory(directory.join("downloads")),
            secrets,
            None,
            Vec::new(),
        )
        .await
        .expect("scheduler")
    }

    /// A hoster grants one free download per IP, and all free flows of one plugin share a
    /// cookie jar. A second concurrent attempt would break the first one's session and earn
    /// both an IP block, so the slot must be strictly serialised — while a different hoster
    /// carries on untouched.
    #[tokio::test]
    async fn a_hoster_runs_one_free_download_at_a_time() {
        let directory = tempfile::tempdir().expect("directory");
        let scheduler = scheduler(directory.path()).await;
        let first_link = url::Url::parse("https://rapidgator.net/file/123456").expect("url");
        let other_hoster =
            url::Url::parse("https://ddownload.com/abc123xyz/release.rar").expect("url");

        let held = scheduler.try_free_slot(&first_link).await;
        assert!(
            matches!(held, ProviderSlot::Acquired(_)),
            "the first free download of a hoster must start"
        );

        assert!(
            matches!(
                scheduler
                    .try_free_slot(
                        &url::Url::parse("https://rapidgator.net/file/654321").expect("url")
                    )
                    .await,
                ProviderSlot::Busy
            ),
            "a second free download of the same hoster must wait"
        );
        assert!(
            matches!(
                scheduler.try_free_slot(&other_hoster).await,
                ProviderSlot::Acquired(_)
            ),
            "another hoster has its own IP allowance and its own slot"
        );

        drop(held);
        assert!(
            matches!(
                scheduler.try_free_slot(&first_link).await,
                ProviderSlot::Acquired(_)
            ),
            "the slot is free again once the download releases it"
        );
    }

    /// Links no free resolver claims are not funnelled through a hoster slot at all.
    #[tokio::test]
    async fn a_link_without_a_free_resolver_is_not_serialised() {
        let directory = tempfile::tempdir().expect("directory");
        let scheduler = scheduler(directory.path()).await;
        let link = url::Url::parse("https://example.test/file.bin").expect("url");

        assert!(matches!(
            scheduler.try_free_slot(&link).await,
            ProviderSlot::Unrestricted
        ));
        assert!(matches!(
            scheduler.try_free_slot(&link).await,
            ProviderSlot::Unrestricted
        ));
    }
}
