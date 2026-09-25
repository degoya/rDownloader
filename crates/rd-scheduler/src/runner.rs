//! Dispatch of non-HTTP files (e.g. Usenet) to pluggable runners inside the shared queue.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use async_trait::async_trait;
use rd_core::{DownloadFile, DownloadKind, DownloadPackage, DownloadState, Failure};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{SchedulerHandle, retry};

/// Result of one runner attempt for a single file.
#[derive(Debug)]
pub enum RunOutcome {
    /// File is complete and stored under `final_name` inside the package destination.
    Completed { final_name: String },
    /// Payload is complete but the engine keeps working in the background (a seeding
    /// torrent); the slot is released now and the engine owns the terminal transition.
    Detached { state: DownloadState },
    /// Cancelled or paused through the cancellation token (state is derived from the request).
    Stopped,
    /// Attempt failed; retry policy follows the failure category.
    Failed(Failure),
}

/// The runtime limits one runner attempt has to work within.
///
/// Bundled rather than passed as two more positional arguments: both values describe the
/// same thing — how much of the machine and the line this one attempt may use.
#[derive(Clone)]
pub struct RunLimits {
    /// Connections one file runner may hold (NNTP, …); `0` lifts the cap and leaves the
    /// transport to its own server limits.
    pub max_parallel_requests: usize,
    /// Byte pacing for this transfer's scope; unlimited when no profile applies.
    pub bandwidth: rd_limits::ScopedLimiter,
}

/// A transport that executes queued files of one kind (segments, articles, …).
#[async_trait]
pub trait ExternalRunner: Send + Sync {
    fn kind(&self) -> DownloadKind;
    /// Files of this kind allowed to run concurrently (e.g. 1 for Usenet).
    fn slot_capacity(&self) -> usize;
    /// Whether running files of this kind occupy one of the `max_active_files` slots.
    /// Open-ended jobs (live recordings) return `false` so they never starve the queue;
    /// their concurrency stays bounded by [`Self::slot_capacity`].
    fn counts_against_global_limit(&self) -> bool {
        true
    }
    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: RunLimits,
    ) -> Result<RunOutcome>;
}

/// Runner registry with a per-kind concurrency semaphore.
#[derive(Default)]
pub(crate) struct RunnerRegistry {
    runners: HashMap<DownloadKind, Arc<dyn ExternalRunner>>,
    slots: Mutex<HashMap<DownloadKind, Arc<Semaphore>>>,
}

impl RunnerRegistry {
    pub(crate) fn new(runners: Vec<Arc<dyn ExternalRunner>>) -> Self {
        Self {
            runners: runners
                .into_iter()
                .map(|runner| (runner.kind(), runner))
                .collect(),
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn get(&self, kind: DownloadKind) -> Option<Arc<dyn ExternalRunner>> {
        self.runners.get(&kind).cloned()
    }

    /// Non-blocking slot acquisition; `None` when the kind is saturated.
    pub(crate) async fn try_slot(&self, kind: DownloadKind) -> Option<OwnedSemaphorePermit> {
        let runner = self.runners.get(&kind)?;
        let semaphore = {
            let mut slots = self.slots.lock().await;
            Arc::clone(
                slots
                    .entry(kind)
                    .or_insert_with(|| Arc::new(Semaphore::new(runner.slot_capacity().max(1)))),
            )
        };
        semaphore.try_acquire_owned().ok()
    }
}

impl SchedulerHandle {
    /// Verifies a package destination has room for `remaining` bytes; `None` means no
    /// runner could state a size and the headroom policy applies.
    ///
    /// Returns `false` when the target was blocked instead. The caller marks the file
    /// `Blocked`, and releasing the root requeues it — no retry is burned.
    pub(crate) async fn ensure_capacity(
        &self,
        destination: &str,
        remaining: Option<u64>,
    ) -> Result<bool> {
        if destination.is_empty() {
            return Ok(true);
        }
        let destination = std::path::Path::new(destination);
        let verdict = match self.capacity().check(destination, remaining).await {
            Ok(verdict) => verdict,
            // A destination that cannot be probed yet (not created, network share down) is
            // left to the runner's own error handling rather than blocking the whole root.
            Err(error) => {
                tracing::debug!(%error, "capacity check skipped for an unprobeable destination");
                return Ok(true);
            }
        };
        let Some(shortfall) = verdict.shortfall() else {
            return Ok(true);
        };
        let target = self.capacity().target_for(destination).await;
        // Through the shared helper, not `set_blocked` alone: a block that is only in memory
        // is invisible to the interface until the next supervision tick and does not survive a
        // restart, so `storage_auto_resume = false` did not actually hold the root.
        self.record_storage_block(target, destination, shortfall)
            .await?;
        Ok(false)
    }

    /// Runs one file through its external runner and maps the outcome to queue states.
    pub(crate) async fn run_external(
        &self,
        runner: Arc<dyn ExternalRunner>,
        file: &DownloadFile,
        cancellation: CancellationToken,
    ) -> Result<()> {
        let package = self
            .database
            .list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == file.package_id)
            .context("download package not found")?;
        if file.state == DownloadState::RetryWait {
            self.database
                .transition_download(file.id, DownloadState::Queued)
                .await?;
        }
        self.database
            .transition_download(file.id, DownloadState::Resolving)
            .await?;
        // Every non-HTTP transport passes through here, so one check covers Usenet,
        // torrent, media, gallery and stream instead of five separate ones.
        let remaining = file
            .total_bytes
            .map(|total| total.get().saturating_sub(file.committed_bytes.get()));
        if !self
            .ensure_capacity(&package.destination, remaining)
            .await?
        {
            self.database
                .transition_download(file.id, DownloadState::Blocked)
                .await?;
            return Ok(());
        }
        self.database
            .transition_download(file.id, DownloadState::Downloading)
            .await?;
        let limits = RunLimits {
            max_parallel_requests: self
                .external_connections_per_file
                .load(std::sync::atomic::Ordering::Acquire),
            bandwidth: self.scoped_limiter(file).await,
        };
        match runner.run(file, &package, cancellation, limits).await? {
            RunOutcome::Completed { final_name } => {
                self.database
                    .transition_download(file.id, DownloadState::Verifying)
                    .await?;
                self.database
                    .complete_download(file.id, final_name, None)
                    .await?;
                Ok(())
            }
            RunOutcome::Detached { state } => {
                self.database.transition_download(file.id, state).await?;
                Ok(())
            }
            RunOutcome::Stopped => crate::worker::transition_stopped(self, file).await,
            RunOutcome::Failed(failure) => {
                let retry_at = retry::retry_at(&failure, file.retry_count, self.max_retries());
                self.database
                    .record_failure(file.id, failure, retry_at)
                    .await?;
                Ok(())
            }
        }
    }
}
