//! Capacity supervision (RD-050-15).
//!
//! Keeps the shared [`CapacityService`] in step with the configured storage roots, blocks a
//! root whose free space fell below its threshold and releases it again. Blocking is
//! per target on purpose: a full media disk must not stop the packages of another root.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use rd_core::{DownloadFile, DownloadState, StorageSettings};
use rd_files::{CapacityService, CapacityShortfall, StorageTarget};

use crate::{BlockReason, SchedulerHandle};

/// Blocks persisted across a restart, so a root does not silently resume while automatic
/// resume is switched off.
const BLOCK_STATE_KEY: &str = "storage.capacity_blocks";

impl SchedulerHandle {
    /// The capacity policy shared with the runners and the REST layer.
    #[must_use]
    pub fn capacity(&self) -> CapacityService {
        self.config.capacity.clone()
    }

    /// Restores persisted blocks and loads the configured roots once at startup.
    pub(crate) async fn restore_capacity(&self) -> Result<()> {
        let stored = self.database.get_setting(BLOCK_STATE_KEY).await?;
        if let Some(stored) = stored {
            match serde_json::from_value::<Vec<(StorageTarget, CapacityShortfall)>>(stored) {
                Ok(blocked) => self.config.capacity.restore(blocked).await,
                Err(error) => {
                    tracing::warn!(%error, "stored storage blocks were unreadable and are ignored");
                }
            }
        }
        self.reload_capacity_config().await
    }

    /// Pushes the current roots, thresholds and fallback directory into the shared service.
    pub async fn reload_capacity_config(&self) -> Result<()> {
        let settings = self.storage_settings().await?;
        let roots = self
            .database
            .list_storage_roots()
            .await?
            .into_iter()
            .map(|root| rd_files::RootLimit {
                id: root.id,
                path: PathBuf::from(root.path),
                minimum_free_bytes: root.minimum_free_bytes.map(rd_core::ByteCount::get),
            })
            .collect();
        self.config
            .capacity
            .apply(
                settings,
                roots,
                Some(self.config.downloads_directory.clone()),
            )
            .await;
        Ok(())
    }

    /// Falls back to defaults rather than refusing: this runs on every supervision cycle, and
    /// an unusable blob must not stop capacity supervision altogether. The defaults are the
    /// documented reserve rather than "no threshold", and the accessor reports the failure.
    async fn storage_settings(&self) -> Result<StorageSettings> {
        self.database.service_settings_or_default().await
    }

    /// Re-probes every target, blocks the ones that fell below their threshold and
    /// releases the ones that have room again.
    pub(crate) async fn supervise_capacity(&self) -> Result<()> {
        self.reload_capacity_config().await?;
        let capacity = self.capacity();
        for (target, path) in capacity.targets().await {
            let free = match capacity.probe(&path).await {
                Ok(free) => free,
                // An unreadable path (a detached network share) is not a capacity verdict;
                // the transfers on it fail with their own error instead.
                Err(error) => {
                    tracing::debug!(%error, path = %path.display(), "free space could not be probed");
                    continue;
                }
            };
            self.supervise_target(target, &path, free).await?;
        }
        Ok(())
    }

    /// One target's supervision step with the free space its probe reported; apart from the
    /// probe, so a test hands in the figure a disk would report.
    pub(crate) async fn supervise_target(
        &self,
        target: StorageTarget,
        path: &Path,
        free: u64,
    ) -> Result<()> {
        let capacity = self.capacity();
        let minimum = capacity.minimum_free_bytes(path).await;
        if free < minimum {
            let shortfall = CapacityShortfall {
                required_bytes: minimum,
                free_bytes: free,
                minimum_free_bytes: minimum,
                size_known: true,
            };
            if self.record_storage_block(target, path, shortfall).await? {
                self.block_transfers_of(target).await;
            }
        } else if capacity.settings().await.storage_auto_resume
            && capacity.is_blocked(target).await
            && self.something_could_start(target, path, free).await?
            && capacity.set_blocked(target, None).await
        {
            tracing::info!(path = %path.display(), free, "storage root released");
            self.requeue_capacity_blocked_of(target).await;
            self.persist_capacity_blocks(CapacityChange::Released, target)
                .await?;
        }
        Ok(())
    }

    /// Whether releasing `target` lets a download start (RD-1270-07): at least one download
    /// waiting on it passes the start check with its remaining size against `free` — the same
    /// rule, [`rd_files::CapacityService::evaluate`], not a second one. An unknown size needs
    /// the threshold times the unknown-size headroom, as at the start.
    ///
    /// Released at the threshold alone, a download that needs more than the gap above it
    /// blocked the root again on its next start, and the root swung between blocked and
    /// released with every supervision round, an event each time, while nothing downloaded.
    /// A target nothing waits on is released at its threshold, as before.
    async fn something_could_start(
        &self,
        target: StorageTarget,
        path: &Path,
        free: u64,
    ) -> Result<bool> {
        let blocked = self
            .database
            .downloads_blocked_by(BlockReason::Capacity.as_str())
            .await?;
        // What the block held back: the transfers it stopped, and the files the dispatch pass
        // leaves waiting while the root is blocked.
        let waiting = self
            .downloads_of(target, |file| {
                blocked.contains(&file.id)
                    || matches!(file.state, DownloadState::Queued | DownloadState::RetryWait)
            })
            .await;
        if waiting.is_empty() {
            return Ok(true);
        }
        let capacity = self.capacity();
        for file in &waiting {
            if capacity
                .evaluate(path, free, remaining_bytes(file))
                .await
                .is_ok()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Records a storage block: marks the target, writes it down and tells the interface.
    ///
    /// The one path both callers take. `ensure_capacity` used to do only the first of the
    /// three, so its block was invisible to the interface until the next supervision tick and
    /// was gone after a restart — which made `storage_auto_resume = false` a promise the
    /// service could not keep, because nothing was left to hold the root.
    ///
    /// Returns whether this call was the one that blocked the target.
    pub(crate) async fn record_storage_block(
        &self,
        target: StorageTarget,
        path: &Path,
        shortfall: CapacityShortfall,
    ) -> Result<bool> {
        if !self
            .config
            .capacity
            .set_blocked(target, Some(shortfall))
            .await
        {
            return Ok(false);
        }
        tracing::warn!(
            path = %path.display(),
            free = shortfall.free_bytes,
            required = shortfall.required_bytes,
            minimum = shortfall.minimum_free_bytes,
            "storage root blocked: not enough free space"
        );
        self.persist_capacity_blocks(CapacityChange::Blocked, target)
            .await?;
        Ok(true)
    }

    /// Explicitly releases a blocked target; used when automatic resume is switched off.
    pub async fn resume_storage(&self, target: StorageTarget) -> Result<bool> {
        if !self.config.capacity.set_blocked(target, None).await {
            return Ok(false);
        }
        self.requeue_capacity_blocked_of(target).await;
        self.persist_capacity_blocks(CapacityChange::Released, target)
            .await?;
        Ok(true)
    }

    /// Writes the blocks down and announces the one transition that led here (RD-1260-04): the
    /// event names it, because a release used to read exactly like a block and was notified as
    /// "Storage root out of space".
    ///
    /// The event also names the target that changed (RD-1270-18): its key (the root id or
    /// `fallback`, what `resume_storage_target` takes), its name and the path its free space is
    /// probed on -- the root's, not the destination a start check was asked for -- and nothing
    /// else: a storage root carries no credential. One event per target: a block or a release is
    /// always one target's transition, so a round that blocks two roots announces two, and an
    /// automation or a notification reads one target, not a list. `blocked` stays the count of
    /// every blocked target.
    async fn persist_capacity_blocks(
        &self,
        change: CapacityChange,
        target: StorageTarget,
    ) -> Result<()> {
        let blocked = self.config.capacity.blocked().await;
        self.database
            .set_setting(BLOCK_STATE_KEY.to_owned(), serde_json::to_value(&blocked)?)
            .await?;
        let name = self.storage_target_name(target).await;
        let path = self
            .config
            .capacity
            .targets()
            .await
            .into_iter()
            .find(|(candidate, _)| *candidate == target)
            .map(|(_, path)| path.to_string_lossy().into_owned());
        self.database.broadcast(rd_core::EventEnvelope::new(
            rd_core::EventKind::StorageCapacity,
            serde_json::json!({
                "blocked": blocked.len(),
                "change": change.as_str(),
                "target": target.key(),
                "name": name,
                "path": path,
            }),
        ));
        Ok(())
    }

    /// The name a target is shown under: the root's own, [`StorageTarget::FALLBACK_NAME`] for
    /// the download directory; `None` for a root that is no longer configured or unreadable.
    async fn storage_target_name(&self, target: StorageTarget) -> Option<String> {
        let StorageTarget::Root(id) = target else {
            return Some(StorageTarget::FALLBACK_NAME.to_owned());
        };
        match self.database.list_storage_roots().await {
            Ok(roots) => roots
                .into_iter()
                .find(|root| root.id == id)
                .map(|root| root.name),
            Err(error) => {
                tracing::warn!(%error, "storage roots were not read for the capacity event");
                None
            }
        }
    }

    /// Requeues the downloads this target's capacity stop blocked, so releasing a root
    /// restarts exactly the work it held back — and nothing else.
    ///
    /// Filtered on the recorded cause, not on `Blocked`: that state is also where a transfer
    /// whose ETag changed mid-flight lands, and restarting that one writes a different file's
    /// bytes over confirmed ones. Free disk space says nothing about either that or a kind the
    /// operator switched off.
    async fn requeue_capacity_blocked_of(&self, target: StorageTarget) {
        let Ok(blocked) = self
            .database
            .downloads_blocked_by(BlockReason::Capacity.as_str())
            .await
        else {
            return;
        };
        self.for_each_download_of(
            target,
            |file| blocked.contains(&file.id),
            |scheduler, id| Box::pin(async move { scheduler.resume(id).await }),
        )
        .await;
    }

    /// Blocks the running transfers of a target that just ran out of space.
    ///
    /// Blocked rather than paused: the partial files are kept either way, but only a block
    /// records why the transfer stopped, and only a recorded reason lets the release restart
    /// these and not somebody else's. Cancelling would throw away work the user can finish.
    async fn block_transfers_of(&self, target: StorageTarget) {
        self.for_each_download_of(
            target,
            |file| {
                matches!(
                    file.state,
                    DownloadState::Downloading | DownloadState::Resolving
                )
            },
            |scheduler, id| {
                Box::pin(async move { scheduler.block(id, BlockReason::Capacity).await })
            },
        )
        .await;
    }

    /// Applies `action` to every download of `target` that `matches` selects.
    async fn for_each_download_of<P, F>(&self, target: StorageTarget, matches: P, action: F)
    where
        P: Fn(&DownloadFile) -> bool,
        F: for<'a> Fn(
            &'a SchedulerHandle,
            rd_core::DownloadId,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>,
        >,
    {
        for file in self.downloads_of(target, matches).await {
            if let Err(error) = action(self, file.id).await {
                tracing::warn!(download_id = %file.id, %error, "capacity action failed");
            }
        }
    }

    /// The downloads of `target` that `matches` selects.
    async fn downloads_of<P>(&self, target: StorageTarget, matches: P) -> Vec<DownloadFile>
    where
        P: Fn(&DownloadFile) -> bool,
    {
        let Ok(destinations) = self.package_destinations().await else {
            return Vec::new();
        };
        let mut selected = Vec::new();
        // Package by package: only the packages on `target` have their files read.
        for (package_id, destination) in destinations {
            if self.config.capacity.target_for(&destination).await != target {
                continue;
            }
            match self.database.downloads_for_package(package_id).await {
                Ok(files) => selected.extend(files.into_iter().filter(&matches)),
                Err(error) => {
                    tracing::warn!(%package_id, %error, "a package's downloads were not read");
                }
            }
        }
        selected
    }

    /// Destination directory of every package, for mapping downloads onto storage targets.
    pub(crate) async fn package_destinations(
        &self,
    ) -> Result<HashMap<rd_core::PackageId, PathBuf>> {
        Ok(destinations_of(&self.database.list_packages().await?))
    }
}

/// The destination directory of each package that has one; the dispatch pass reads the
/// packages once for this and for their "not before" (RD-1240-14).
pub(crate) fn destinations_of(
    packages: &[rd_core::DownloadPackage],
) -> HashMap<rd_core::PackageId, PathBuf> {
    packages
        .iter()
        .filter(|package| !package.destination.is_empty())
        .map(|package| (package.id, PathBuf::from(&package.destination)))
        .collect()
}

/// The bytes a download still has to write, `None` while its size is unknown: the size the
/// start check is asked with.
pub(crate) fn remaining_bytes(file: &DownloadFile) -> Option<u64> {
    file.total_bytes
        .map(|total| total.get().saturating_sub(file.committed_bytes.get()))
}

/// Which transition a `storage.capacity` event announces.
#[derive(Clone, Copy)]
enum CapacityChange {
    Blocked,
    Released,
}

impl CapacityChange {
    fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Released => "released",
        }
    }
}

#[cfg(test)]
#[path = "capacity_tests.rs"]
mod tests;
