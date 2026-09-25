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
        let settings = capacity.settings().await;
        for (target, path) in capacity.targets().await {
            let minimum = capacity.minimum_free_bytes(&path).await;
            let free = match capacity.probe(&path).await {
                Ok(free) => free,
                // An unreadable path (a detached network share) is not a capacity verdict;
                // the transfers on it fail with their own error instead.
                Err(error) => {
                    tracing::debug!(%error, path = %path.display(), "free space could not be probed");
                    continue;
                }
            };
            if free < minimum {
                let shortfall = CapacityShortfall {
                    required_bytes: minimum,
                    free_bytes: free,
                    minimum_free_bytes: minimum,
                    size_known: true,
                };
                if self.record_storage_block(target, &path, shortfall).await? {
                    self.block_transfers_of(target).await;
                }
            } else if settings.storage_auto_resume && capacity.set_blocked(target, None).await {
                tracing::info!(path = %path.display(), free, "storage root released");
                self.requeue_capacity_blocked_of(target).await;
                self.persist_capacity_blocks().await?;
            }
        }
        Ok(())
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
        self.persist_capacity_blocks().await?;
        Ok(true)
    }

    /// Explicitly releases a blocked target; used when automatic resume is switched off.
    pub async fn resume_storage(&self, target: StorageTarget) -> Result<bool> {
        if !self.config.capacity.set_blocked(target, None).await {
            return Ok(false);
        }
        self.requeue_capacity_blocked_of(target).await;
        self.persist_capacity_blocks().await?;
        Ok(true)
    }

    async fn persist_capacity_blocks(&self) -> Result<()> {
        let blocked = self.config.capacity.blocked().await;
        self.database
            .set_setting(BLOCK_STATE_KEY.to_owned(), serde_json::to_value(&blocked)?)
            .await?;
        self.database.broadcast(rd_core::EventEnvelope::new(
            rd_core::EventKind::StorageCapacity,
            serde_json::json!({ "blocked": blocked.len() }),
        ));
        Ok(())
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
        let Ok(destinations) = self.package_destinations().await else {
            return;
        };
        let Ok(files) = self.database.list_downloads().await else {
            return;
        };
        for file in files.into_iter().filter(&matches) {
            let Some(destination) = destinations.get(&file.package_id) else {
                continue;
            };
            if self.config.capacity.target_for(destination).await != target {
                continue;
            }
            if let Err(error) = action(self, file.id).await {
                tracing::warn!(download_id = %file.id, %error, "capacity action failed");
            }
        }
    }

    /// Destination directory of every package, for mapping downloads onto storage targets.
    pub(crate) async fn package_destinations(
        &self,
    ) -> Result<HashMap<rd_core::PackageId, PathBuf>> {
        Ok(self
            .database
            .list_packages()
            .await?
            .into_iter()
            .filter(|package| !package.destination.is_empty())
            .map(|package| (package.id, PathBuf::from(package.destination)))
            .collect())
    }
}
