//! Dispatch of non-HTTP files (e.g. Usenet) to pluggable runners inside the shared queue.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use rd_core::{DownloadFile, DownloadKind, DownloadPackage, DownloadState, Failure};
use tokio::sync::Mutex;
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
    /// Files of this kind allowed to run concurrently.
    fn slot_capacity(&self) -> usize;
    /// Files of this kind allowed to run right now, asked on every dispatch pass.
    ///
    /// `requested_files` is the operator's parallel-file setting, `0` for "let the runner
    /// decide" (RD-130-22). A runner whose right number depends on its own load - Usenet,
    /// which sizes it by the articles its running files still hold - answers here; every
    /// other one keeps its fixed [`Self::slot_capacity`].
    fn dispatch_capacity(&self, _requested_files: usize) -> usize {
        self.slot_capacity()
    }
    /// Whether running files of this kind occupy one of the `max_active_files` slots.
    /// Open-ended jobs (live recordings) return `false` so they never starve the queue;
    /// their concurrency stays bounded by [`Self::slot_capacity`].
    fn counts_against_global_limit(&self) -> bool {
        true
    }
    /// Whether all running files of this kind together occupy a single `max_active_files`
    /// slot, because they are one transfer: files that share one connection pool (Usenet)
    /// add work to it, not connections (RD-130-22). Their number stays bounded by
    /// [`Self::dispatch_capacity`].
    fn shares_one_global_slot(&self) -> bool {
        false
    }
    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: RunLimits,
    ) -> Result<RunOutcome>;
}

/// Runner registry with a per-kind count of running files.
#[derive(Default)]
pub(crate) struct RunnerRegistry {
    runners: HashMap<DownloadKind, Arc<dyn ExternalRunner>>,
    /// Files of each kind running right now. A count rather than a semaphore sized once,
    /// because a runner's capacity may change between two passes (RD-130-22).
    running: Mutex<HashMap<DownloadKind, Arc<AtomicUsize>>>,
}

/// One running file's place in its kind's count; given back when it is dropped.
pub(crate) struct KindSlot(Arc<AtomicUsize>);

impl Drop for KindSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl RunnerRegistry {
    pub(crate) fn new(runners: Vec<Arc<dyn ExternalRunner>>) -> Self {
        Self {
            runners: runners
                .into_iter()
                .map(|runner| (runner.kind(), runner))
                .collect(),
            running: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn get(&self, kind: DownloadKind) -> Option<Arc<dyn ExternalRunner>> {
        self.runners.get(&kind).cloned()
    }

    /// Non-blocking slot acquisition; `None` when the kind is saturated.
    ///
    /// `requested_files` is handed to [`ExternalRunner::dispatch_capacity`]. The count is
    /// compared and raised under the registry's lock, so two passes cannot both take the
    /// last place; it only ever falls outside the lock, which errs on the safe side.
    pub(crate) async fn try_slot(
        &self,
        kind: DownloadKind,
        requested_files: usize,
    ) -> Option<KindSlot> {
        let runner = self.runners.get(&kind)?;
        let mut running = self.running.lock().await;
        let count = Arc::clone(running.entry(kind).or_default());
        if count.load(Ordering::Acquire) >= runner.dispatch_capacity(requested_files).max(1) {
            return None;
        }
        count.fetch_add(1, Ordering::AcqRel);
        Some(KindSlot(count))
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

#[cfg(test)]
mod registry_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use rd_core::{DownloadFile, DownloadKind, DownloadPackage};
    use tokio_util::sync::CancellationToken;

    use super::{ExternalRunner, RunLimits, RunOutcome, RunnerRegistry};

    /// A runner whose capacity is whatever the test says it is at the moment.
    struct Elastic(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl ExternalRunner for Elastic {
        fn kind(&self) -> DownloadKind {
            DownloadKind::Usenet
        }

        fn slot_capacity(&self) -> usize {
            8
        }

        fn dispatch_capacity(&self, requested_files: usize) -> usize {
            if requested_files > 0 {
                requested_files
            } else {
                self.0.load(Ordering::Acquire)
            }
        }

        async fn run(
            &self,
            _file: &DownloadFile,
            _package: &DownloadPackage,
            _cancellation: CancellationToken,
            _limits: RunLimits,
        ) -> anyhow::Result<RunOutcome> {
            Ok(RunOutcome::Stopped)
        }
    }

    #[tokio::test]
    async fn the_capacity_is_asked_on_every_pass_and_places_come_back() {
        let capacity = Arc::new(AtomicUsize::new(2));
        let registry = RunnerRegistry::new(vec![Arc::new(Elastic(Arc::clone(&capacity)))]);
        let first = registry.try_slot(DownloadKind::Usenet, 0).await;
        let second = registry.try_slot(DownloadKind::Usenet, 0).await;
        assert!(first.is_some() && second.is_some());
        assert!(registry.try_slot(DownloadKind::Usenet, 0).await.is_none());

        // The runner decides it can take a third: the next pass gives it one.
        capacity.store(3, Ordering::Release);
        let third = registry.try_slot(DownloadKind::Usenet, 0).await;
        assert!(third.is_some());
        assert!(registry.try_slot(DownloadKind::Usenet, 0).await.is_none());

        // Down to one: nothing new starts until the running files are below it.
        capacity.store(1, Ordering::Release);
        drop(first);
        assert!(registry.try_slot(DownloadKind::Usenet, 0).await.is_none());
        drop(second);
        drop(third);
        assert!(registry.try_slot(DownloadKind::Usenet, 0).await.is_some());
    }

    #[tokio::test]
    async fn the_operators_number_reaches_the_runner() {
        let registry = RunnerRegistry::new(vec![Arc::new(Elastic(Arc::new(AtomicUsize::new(8))))]);
        let only = registry.try_slot(DownloadKind::Usenet, 1).await;
        assert!(only.is_some());
        assert!(registry.try_slot(DownloadKind::Usenet, 1).await.is_none());
    }

    #[tokio::test]
    async fn a_kind_without_a_runner_gets_no_slot() {
        let registry = RunnerRegistry::new(Vec::new());
        assert!(registry.try_slot(DownloadKind::Torrent, 0).await.is_none());
    }
}
