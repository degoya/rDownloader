//! The supervise loop and the dispatch pass that starts queued files, one attempt each.

use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use anyhow::Result;
use rd_core::{DownloadId, DownloadState, Failure, FailureKind, PackageId};
use tokio_util::sync::CancellationToken;

use crate::{
    BlockReason, ExternalRunner, SchedulerHandle, mirrors, provider::ProviderSlot, rates, worker,
};

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
        if self.pause_during_postprocess.load(Ordering::Acquire)
            && self.config.postprocess_hold.is_held()
        {
            return Ok(());
        }
        // An exhausted traffic budget holds back new starts only; transfers already running
        // finish, so nothing is thrown away at the period boundary. Battery and metered
        // operation hold the queue the same way.
        if self.config.bandwidth.budget_exceeded().await.is_some()
            || self.network_hold().await.is_some()
        {
            return Ok(());
        }
        let now = chrono::Utc::now();
        // The active profile may cap parallelism more tightly than the base setting.
        let active_limit = self
            .config
            .bandwidth
            .max_active_files()
            .await
            .map(|value| value as usize)
            .map_or_else(
                || self.max_active_files.load(Ordering::Acquire),
                |profile_limit| profile_limit.min(self.max_active_files.load(Ordering::Acquire)),
            );
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
            // A hoster's free-download limit applies to the whole IP, so hold back its
            // other anonymous links instead of spending another wait and captcha on them.
            // Downloads backed by an account are unaffected.
            if file.account_id.is_none()
                && self.host_blocks.blocked_until(&file.source, now).is_some()
            {
                continue;
            }
            // One link of a mirror group at a time. Enqueueing already picks the member that
            // runs; this catches the case where somebody started a waiting one by hand, and
            // stands the loser down rather than fetching the same bytes twice.
            if file.mirror_group.is_some() {
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    groups.entry(file.package_id)
                {
                    slot.insert(self.database.downloads_for_package(file.package_id).await?);
                }
                let siblings = groups
                    .get(&file.package_id)
                    .map(|members| mirrors::siblings(&file, members))
                    .unwrap_or_default();
                let taken = siblings
                    .iter()
                    .any(|sibling| mirrors::has_taken_the_turn(sibling.state));
                // Decided by `best_candidate` rather than by which one this loop reached
                // first, so two links added together always resolve the same way round.
                let mut contenders: Vec<&rd_core::DownloadFile> = siblings
                    .into_iter()
                    .filter(|sibling| mirrors::is_contending(sibling.state))
                    .collect();
                contenders.push(&file);
                let loses =
                    mirrors::best_candidate(&contenders).is_some_and(|winner| winner.id != file.id);
                if taken || loses {
                    self.database
                        .transition_download(file.id, DownloadState::Skipped)
                        .await?;
                    // Read again on the next member of the group, which must see this one
                    // standing by rather than contending.
                    groups.remove(&file.package_id);
                    continue;
                }
            }
            // A storage root below its threshold holds back only its own packages; every
            // other destination keeps downloading.
            if let Some(destination) = destinations.get(&file.package_id) {
                let target = self.config.capacity.target_for(destination).await;
                if self.config.capacity.is_blocked(target).await {
                    continue;
                }
            }
            // A switched-off service must not leave work waiting forever with no reason
            // shown, so the job is blocked instead of skipped.
            if self.kind_disabled(file.kind).await {
                if !blocked_kinds.contains(&file.kind) {
                    blocked_kinds.push(file.kind);
                    self.block_queued_of_kind(file.kind).await;
                }
                continue;
            }
            let external = match file.kind {
                rd_core::DownloadKind::Http => None,
                kind => {
                    let Some(runner) = self.runners.get(kind) else {
                        continue;
                    };
                    let requested = self.external_parallel_files.load(Ordering::Acquire);
                    let Some(permit) = self.runners.try_slot(kind, requested).await else {
                        continue;
                    };
                    Some((runner, permit))
                }
            };
            let provider_permit = if external.is_some() {
                None
            } else {
                match self
                    .try_provider_slot(file.id, file.account_id, &file.source)
                    .await?
                {
                    ProviderSlot::Unrestricted => None,
                    ProviderSlot::Acquired(permit) => Some(permit),
                    ProviderSlot::Busy => continue,
                }
            };
            let exempt = external
                .as_ref()
                .is_some_and(|(runner, _)| !runner.counts_against_global_limit());
            let pooled = external
                .as_ref()
                .is_some_and(|(runner, _)| runner.shares_one_global_slot());
            let cancellation = CancellationToken::new();
            {
                let mut active = self.active.lock().await;
                // Asked under the lock `shutdown` collects the tokens under: a pass that was
                // already running when the shutdown began would otherwise add a token nobody
                // cancels and start a job while the WAL is checkpointed (audit 1.9.1, TR-06).
                if self.shutdown.is_cancelled() {
                    return Ok(());
                }
                if active.untouchable(&file.id) {
                    continue;
                }
                // Exempt kinds (recordings) start regardless of the global cap, and so does
                // another file of a pooled kind that is running already, so keep scanning
                // instead of breaking when the cap is reached.
                if !active.admits(file.kind, exempt, pooled, active_limit) {
                    continue;
                }
                active.tokens.insert(file.id, cancellation.clone());
                if exempt {
                    active.exempt.insert(file.id);
                } else if pooled {
                    active.pooled.insert(file.id, file.kind);
                }
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
                && (matches!(
                    current.state,
                    DownloadState::Resolving
                        | DownloadState::Downloading
                        | DownloadState::Verifying
                        | DownloadState::Repairing
                        | DownloadState::Extracting
                ) || (panicked
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
