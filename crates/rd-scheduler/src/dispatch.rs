//! The supervise loop and the dispatch pass that starts queued files, one attempt each.

use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::Result;
use rd_core::{DownloadId, DownloadState, Failure, FailureKind, PackageId};
use tokio_util::sync::CancellationToken;

use crate::{BlockReason, ExternalRunner, SchedulerHandle, rates, worker};

#[path = "dispatch_admission.rs"]
mod admission;

use admission::{Claim, Slots};

impl SchedulerHandle {
    pub(crate) async fn supervise(self) {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        let mut capacity_tick: u64 = 0;
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                _ = ticker.tick() => {
                    capacity_tick = capacity_tick.wrapping_add(1);
                    // Free space changes slowly compared to the dispatch loop, so it is
                    // probed every fourth tick instead of twice a second.
                    if capacity_tick.is_multiple_of(4)
                        && let Err(error) = self.supervise_capacity().await
                    {
                        tracing::error!(%error, "storage capacity supervision failed");
                    }
                    // The schedule is evaluated every fifteen seconds; a window boundary is
                    // a minute-grained event, so this is far finer than it needs to be.
                    if capacity_tick.is_multiple_of(30)
                        && let Err(error) = self.supervise_bandwidth().await
                    {
                        tracing::error!(%error, "bandwidth supervision failed");
                    }
                    // Once a second. The traffic budget above measures queue-wide totals every
                    // fifteen seconds, which says nothing about how fast one entry is moving.
                    if capacity_tick.is_multiple_of(2)
                        && let Err(error) = self.supervise_rates().await
                    {
                        tracing::error!(%error, "transfer rate sampling failed");
                    }
                    // Once a minute: its intervals are counted in hours (RD-191-12).
                    if capacity_tick.is_multiple_of(120)
                        && let Err(error) = self.supervise_auto_retry().await
                    {
                        tracing::error!(%error, "the automatic retry of failed downloads failed");
                    }
                    // Every tick, and before the dispatch below: the end of a pause is the
                    // moment its files may start, not up to a second later.
                    if let Err(error) = self.supervise_queue_pause().await {
                        tracing::error!(%error, "the timed queue pause could not be ended");
                    }
                    self.supervise_account_traffic().await;
                    // Before the dispatch below as well: once the marked file or package is
                    // done, this tick's pass must already find the queue held (RD-1210-02).
                    if let Err(error) = self.supervise_stop_mark().await {
                        tracing::error!(%error, "the stop mark could not be checked");
                    }
                    if let Err(error) = self.schedule_runnable().await {
                        tracing::error!(%error, "queue supervision failed");
                    }
                }
            }
        }
    }

    /// Folds the current byte counters into the smoothed per-download rates.
    ///
    /// Its own read of the queue rather than the dispatch loop's: that one returns early while
    /// a budget, a hold or post-processing keeps the queue back, and a rate frozen at whatever
    /// it was when the hold began would be worse than one that decays to nothing.
    ///
    /// Only the files that hold a slot are read, one row each: only those move bytes, and the
    /// whole table once a second was most of what an idle queue cost (audit 1.9.1, TR-08).
    async fn supervise_rates(&self) -> Result<()> {
        let running = self
            .active
            .lock()
            .await
            .tokens
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let mut observations = Vec::with_capacity(running.len());
        for id in running {
            let Some(file) = self.database.get_download(id).await? else {
                continue;
            };
            observations.push(rates::RateObservation {
                id: file.id,
                committed_bytes: file.committed_bytes.get(),
                // Only a running transfer moves bytes. Verifying, repairing, extracting and
                // seeding do not, and neither does anything that is waiting.
                transferring: file.state == DownloadState::Downloading,
            });
        }
        self.rates.observe(std::time::Instant::now(), &observations);
        Ok(())
    }

    /// The current smoothed rate of every running download, in bytes per second.
    ///
    /// Empty until the supervise loop has sampled twice; a rate needs two readings to exist.
    /// A download that holds no slot has no entry.
    #[must_use]
    pub fn transfer_rates(&self) -> HashMap<DownloadId, u64> {
        self.rates.rates()
    }

    pub(crate) async fn schedule_runnable(&self) -> Result<()> {
        // Rebuilt by every pass, so a file that was paused, removed or started meanwhile stops
        // being listed as waiting for its host, and a held queue lists none.
        let mut host_waits = HashMap::new();
        let result = self.dispatch_pass(&mut host_waits).await;
        self.active
            .lock()
            .await
            .host
            .settle(host_waits, std::time::Instant::now());
        result
    }

    /// One pass over the startable files; `host_waits` collects the ones held back because
    /// their host has no free connection (RD-1130-02).
    async fn dispatch_pass(&self, host_waits: &mut HashMap<DownloadId, String>) -> Result<()> {
        if self.dispatch_held().await {
            return Ok(());
        }
        let now = chrono::Utc::now();
        let active_limit = self.active_limit().await;
        // Only the rows that can start, through the state index: an idle queue of finished
        // downloads used to be loaded whole, JSON and all, twice a second (audit 1.9.1, TR-08).
        let files = self
            .startable_downloads()
            .await?
            .into_iter()
            .filter(|file| {
                file.state == DownloadState::Queued
                    || file.next_retry_at.is_some_and(|retry_at| retry_at <= now)
            })
            .collect::<Vec<_>>();
        if files.is_empty() {
            return Ok(());
        }
        let destinations = self.package_destinations().await?;
        // A mirror group's members, read once per package and pass: the check below needs a
        // file's siblings in every state, not only the startable ones.
        let mut groups: HashMap<PackageId, Vec<rd_core::DownloadFile>> = HashMap::new();
        // A switched-off kind is blocked once per pass, not once per waiting file of it.
        let mut blocked_kinds: Vec<rd_core::DownloadKind> = Vec::new();
        for file in files {
            if !self
                .may_start(&file, now, &destinations, &mut groups, &mut blocked_kinds)
                .await?
            {
                continue;
            }
            let host = self.host_claim(&file);
            let Some(Slots {
                external,
                provider_permit,
            }) = self.acquire_slots(&file).await?
            else {
                continue;
            };
            let exempt = external
                .as_ref()
                .is_some_and(|(runner, _)| !runner.counts_against_global_limit());
            let pooled = external
                .as_ref()
                .is_some_and(|(runner, _)| runner.shares_one_global_slot());
            let cancellation = CancellationToken::new();
            match self
                .claim_slot(&file, exempt, pooled, active_limit, host, &cancellation)
                .await
            {
                Claim::ShuttingDown => return Ok(()),
                Claim::Refused => continue,
                Claim::HostBusy(host) => {
                    host_waits.insert(file.id, host);
                    continue;
                }
                Claim::Claimed => {}
            }
            let scheduler = self.clone();
            tokio::spawn(async move {
                let _provider_permit = provider_permit;
                let _kind_permit = external.as_ref().map(|(_, permit)| permit);
                let runner = external.as_ref().map(|(runner, _)| Arc::clone(runner));
                scheduler.run_file(file, cancellation, runner).await;
            });
        }
        Ok(())
    }

    /// Whether the queue holds back new starts this pass.
    async fn dispatch_held(&self) -> bool {
        if self.pause_during_postprocess.load(Ordering::Acquire)
            && self.config.postprocess_hold.is_held()
        {
            return true;
        }
        // An exhausted traffic budget holds back new starts only; transfers already running
        // finish, so nothing is thrown away at the period boundary. Battery and metered
        // operation hold the queue the same way.
        self.config.bandwidth.budget_exceeded().await.is_some()
            || self.network_hold().await.is_some()
    }

    /// How many files may run at once.
    async fn active_limit(&self) -> usize {
        // The active profile may cap parallelism more tightly than the base setting.
        self.config
            .bandwidth
            .max_active_files()
            .await
            .map(|value| value as usize)
            .map_or_else(
                || self.max_active_files.load(Ordering::Acquire),
                |profile_limit| profile_limit.min(self.max_active_files.load(Ordering::Acquire)),
            )
    }

    /// Whether a kind is currently switched off.
    async fn kind_disabled(&self, kind: rd_core::DownloadKind) -> bool {
        self.disabled_kinds.lock().await.contains(&kind)
    }

    /// Puts every queued job of a now-disabled kind into `Blocked`.
    ///
    /// Blocking rather than skipping: a job that is silently passed over on every pass looks
    /// identical to one that is merely waiting its turn, and there is nothing in the queue
    /// that says why it never starts.
    async fn block_queued_of_kind(&self, kind: rd_core::DownloadKind) {
        let files = match self.startable_downloads().await {
            Ok(files) => files,
            Err(error) => {
                // Said rather than swallowed (audit 1.9.1, TR-18): the next pass tries again,
                // but a database that keeps refusing should show up in the log.
                tracing::warn!(%error, ?kind, "queued jobs of a disabled kind were not read");
                return;
            }
        };
        for file in files
            .into_iter()
            .filter(|file| file.kind == kind && file.state == DownloadState::Queued)
        {
            if let Err(error) = self
                .database
                .block_download(file.id, BlockReason::KindDisabled.as_str())
                .await
            {
                tracing::warn!(%error, download = %file.id, "could not block a disabled kind");
            }
        }
    }

    /// The rows a dispatch pass may start, read through the state index.
    async fn startable_downloads(&self) -> Result<Vec<rd_core::DownloadFile>> {
        #[cfg(test)]
        self.queue_reads.fetch_add(1, Ordering::AcqRel);
        self.database.startable_downloads().await
    }

    /// One attempt on `file` and everything that has to follow it, whatever the attempt did.
    ///
    /// The attempt runs in a task of its own, so a panic in a worker or a runner ends that task
    /// and nothing else: the slot below is given back and the row is recorded as a failed
    /// attempt, where it used to keep its place in `active` and sit in `Downloading` until the
    /// next start (audit 1.9.1, TR-05).
    async fn run_file(
        &self,
        file: rd_core::DownloadFile,
        cancellation: CancellationToken,
        runner: Option<Arc<dyn ExternalRunner>>,
    ) {
        let attempt = {
            let scheduler = self.clone();
            let file = file.clone();
            tokio::spawn(async move { scheduler.attempt(&file, cancellation, runner).await })
        };
        let (result, panicked) = match attempt.await {
            Ok(result) => (result, false),
            Err(error) => (
                Err(anyhow::anyhow!(
                    "the download attempt ended abnormally: {error}"
                )),
                true,
            ),
        };
        if let Err(error) = &result {
            // With its causes: the top line of a request error is "error sending request",
            // and what went wrong sits further down (audit 1.9.1, TR-12).
            let message = format!("{error:#}");
            tracing::warn!(download_id = %file.id, error = %message, "download attempt failed");
            if let Ok(Some(current)) = self.database.get_download(file.id).await
                && (current.state.is_working()
                    || (panicked
                    // A panic before the first transition left the row startable; without a
                    // recorded attempt the next pass would run into the same panic at once.
                    && matches!(
                        current.state,
                        DownloadState::Queued | DownloadState::RetryWait
                    )))
            {
                let failure = Failure::new(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    message,
                );
                // The same way as a failure the runner reported: a mirror group hands its turn
                // on once the attempts are spent, where `record_failure` alone left the waiting
                // members `Skipped` until the next start (re-audit 1.9.1, RA-TR-01) — a missing
                // `yt-dlp` is an `Err`, not a `RunOutcome::Failed`.
                if let Err(error) = crate::failures::record_error(self, &current, failure).await {
                    // The token is dropped just below either way, so a lost write leaves the
                    // row in `Downloading`/`Resolving` with nothing running behind it, and
                    // `schedule_runnable` only ever looks at `Queued`/`RetryWait`. The entry
                    // is then dead until the next `recover_interrupted`; say so.
                    tracing::warn!(
                        %error,
                        download_id = %file.id,
                        "could not record the failure of a crashed attempt"
                    );
                }
            }
        }
        {
            let mut active = self.active.lock().await;
            active.tokens.remove(&file.id);
            active.reasons.remove(&file.id);
            active.exempt.remove(&file.id);
            active.pooled.remove(&file.id);
            active.host.finished(&file.id);
        }
        // A category change that had to leave this file behind can carry on now that it is no
        // longer running; the last file of the package sweeps the former directory. Cheap and
        // silent when the package has no outstanding move, which is the normal case.
        if let Err(error) = self.relocate_package(file.package_id).await {
            tracing::warn!(
                package_id = %file.package_id,
                %error,
                "outstanding category move was not completed"
            );
        }
    }

    /// The attempt itself: the external runner, or the built-in HTTP worker.
    async fn attempt(
        &self,
        file: &rd_core::DownloadFile,
        cancellation: CancellationToken,
        runner: Option<Arc<dyn ExternalRunner>>,
    ) -> Result<()> {
        // The trace every attempt on this download belongs to (RD-110-03).
        //
        // Derived from the download id rather than inherited from whoever enqueued it: queued
        // work outlives the request that queued it — this runs minutes later, in another
        // task, possibly after a restart — so there is no request context left to inherit.
        // Deriving means the resolver call, the transfer and post-processing all land in one
        // trace for download `42` without a single function growing a parameter, because
        // `rd_diagnostics` copies an open span's `trace_id` onto everything inside it.
        let trace = rd_core::TraceContext::for_job("download", &file.id.to_string());
        let span = tracing::info_span!(
            "download.run",
            trace_id = %trace.trace_id_hex(),
            download_id = %file.id,
            kind = ?file.kind,
        );
        tracing::Instrument::instrument(
            async {
                match runner {
                    Some(runner) => self.run_external(runner, file, cancellation).await,
                    None => worker::run(self, file, cancellation).await,
                }
            },
            span,
        )
        .await
    }
}
