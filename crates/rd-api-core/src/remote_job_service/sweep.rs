//! The sweep that drives every remote job through its plugin until the files are queued.

use super::*;

impl RemoteJobService {
    pub(super) async fn sweep_loop(self) {
        let mut ticker = tokio::time::interval(SWEEP);
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }
            if let Err(error) = self.sweep_once(Utc::now()).await {
                tracing::warn!(%error, "remote jobs could not be swept");
            }
        }
    }

    /// One pass over every due row, in order and one at a time.
    ///
    /// Sequential on purpose. Two ticks never overlap because the loop awaits this, and rows
    /// are not driven concurrently because the request budget they spend is one account's;
    /// a job that takes a while to answer delays the next one by that while and nothing else.
    pub(crate) async fn sweep_once(&self, now: DateTime<Utc>) -> anyhow::Result<()> {
        let due = self.inner.database.due_remote_jobs(now).await?;
        if due.is_empty() {
            return Ok(());
        }
        let accounts = self.inner.database.list_accounts().await?;
        let runners = self.runners().await?;
        for job in due {
            if !job.state.is_polled() {
                // The query already excludes these; the rule is restated here so a change to
                // the query cannot quietly start polling a job that waits for a person.
                continue;
            }
            let outcome = if accounts.iter().any(|account| account.id == job.account_id) {
                self.drive(&runners, &job, now).await
            } else {
                self.fail(
                    &job,
                    NO_ACCOUNT,
                    "the account this job runs on no longer exists",
                )
                .await
            };
            if let Err(error) = outcome {
                tracing::warn!(remote_job = %job.id, %error, "remote job could not be advanced");
            }
        }
        Ok(())
    }

    /// The one step `submit_step` allows for this row.
    pub(super) async fn drive(
        &self,
        runners: &RemoteJobRunners,
        job: &RemoteJob,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        match job.submit_step() {
            SubmitStep::Submit => self.submit_row(runners, job, now).await,
            SubmitStep::Adopt => self.adopt_row(runners, job, now).await,
            SubmitStep::GiveUp => {
                self.fail(
                    job,
                    SUBMIT_UNCONFIRMED,
                    "two attempts produced no job this installation could name; nothing more is sent",
                )
                .await
            }
            SubmitStep::Poll => {
                let Some(handle) = handle_of(job) else {
                    return self
                        .fail(
                            job,
                            MISSING_REMOTE_ID,
                            "the row names no job at the provider",
                        )
                        .await;
                };
                // A recorded choice means the question was answered; one asked again ends
                // the job rather than reopening it (RD-120-35).
                let outcome = if job.chosen.is_empty() {
                    runners.poll(&job.plugin_id, job.account_id, &handle).await
                } else {
                    runners
                        .poll_answered(&job.plugin_id, job.account_id, &handle)
                        .await
                };
                self.settle(job, outcome, now).await
            }
        }
    }

    /// Hands the source over, once.
    pub(super) async fn submit_row(
        &self,
        runners: &RemoteJobRunners,
        job: &RemoteJob,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let Some(bytes) = self.inner.database.remote_job_source(job.id).await? else {
            // The row vanished between the read and now. Nothing to do and nothing to say.
            return Ok(());
        };
        let source = match job.source_kind {
            RemoteJobSourceKind::Magnet => {
                RemoteJobSource::Magnet(String::from_utf8_lossy(&bytes).into_owned())
            }
            RemoteJobSourceKind::Container => RemoteJobSource::Container(bytes),
            RemoteJobSourceKind::Address => {
                RemoteJobSource::Address(String::from_utf8_lossy(&bytes).into_owned())
            }
        };
        // Counted before the request goes out, not after the answer comes back. A crash in
        // between must show as an attempt, or the next tick would submit a second time
        // instead of asking the provider what it already holds.
        self.inner
            .database
            .advance_remote_job(
                job.id,
                AdvanceRemoteJob {
                    count_submit_attempt: true,
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        match runners
            .submit_named(
                &job.plugin_id,
                job.account_id,
                &source,
                &job.content_key,
                container_name(job),
            )
            .await
        {
            Ok(handle) => self.named(job, handle, false, now).await,
            Err(refusal) => self.refuse(job, refusal, now).await,
        }
    }

    /// Asks the provider what it already holds for this content, before anything is sent a
    /// second time.
    pub(super) async fn adopt_row(
        &self,
        runners: &RemoteJobRunners,
        job: &RemoteJob,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        match runners
            .adopt(&job.plugin_id, job.account_id, &job.content_key)
            .await
        {
            Ok(Some(handle)) => self.named(job, handle, true, now).await,
            Ok(None) => {
                // Nothing there. The row stays due; the next tick reads the count and either
                // submits once more or gives up, and `submit_step` is where that is decided.
                self.inner
                    .database
                    .advance_remote_job(
                        job.id,
                        AdvanceRemoteJob {
                            adoption_checked: true,
                            ..AdvanceRemoteJob::default()
                        },
                    )
                    .await?;
                Ok(())
            }
            Err(refusal) => self.refuse(job, refusal, now).await,
        }
    }

    /// The provider named the job. Written as the very next thing, before anything else.
    pub(super) async fn named(
        &self,
        job: &RemoteJob,
        handle: RemoteJobHandle,
        adopted: bool,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        self.inner
            .database
            .advance_remote_job(
                job.id,
                AdvanceRemoteJob {
                    remote_id: Some(handle.remote_id),
                    job_state: Some(handle.job_state),
                    adoption_checked: adopted,
                    state: Some(RemoteJobState::Preparing),
                    code: Some(None),
                    message: Some(None),
                    next_poll_at: Some(Some(due_at(now, RemoteJobState::Preparing, None))),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(())
    }

    /// Writes what one poll said. Every arm maps to one `AdvanceRemoteJob`, and the store
    /// refuses the ones the row's state does not allow -- a late answer for a job somebody
    /// deleted ends here as an error, not as a resurrected row.
    pub(super) async fn settle(
        &self,
        job: &RemoteJob,
        outcome: PollOutcome,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let advance = match outcome {
            PollOutcome::Preparing {
                retry_after_seconds,
            } => AdvanceRemoteJob {
                state: Some(RemoteJobState::Preparing),
                code: Some(None),
                message: Some(None),
                next_poll_at: Some(Some(due_at(
                    now,
                    RemoteJobState::Preparing,
                    retry_after_seconds,
                ))),
                ..AdvanceRemoteJob::default()
            },
            PollOutcome::AwaitingChoice(entries) => AdvanceRemoteJob {
                state: Some(RemoteJobState::AwaitingChoice),
                entries: Some(entries),
                code: Some(None),
                message: Some(None),
                // The one state that is open and not polled: nothing at the provider changes
                // until a person answers, and `choose` is what puts a time back here.
                next_poll_at: Some(None),
                ..AdvanceRemoteJob::default()
            },
            PollOutcome::Working(work) => AdvanceRemoteJob {
                state: Some(RemoteJobState::Working),
                progress_permille: Some(work.progress_permille),
                code: Some(None),
                message: Some(None),
                next_poll_at: Some(Some(due_at(now, RemoteJobState::Working, None))),
                ..AdvanceRemoteJob::default()
            },
            PollOutcome::Ready(artifacts) => return self.hand_over(job, artifacts).await,
            PollOutcome::Refused(refusal) => return self.refuse(job, refusal, now).await,
        };
        self.inner
            .database
            .advance_remote_job(job.id, advance)
            .await?;
        Ok(())
    }

    /// What a finished job produced goes to the LinkGrabber, and then the row is closed.
    ///
    /// One job is one batch, and a batch's packages are its own: nothing in the LinkGrabber
    /// merges packages across batches, so two jobs never share a package however alike their
    /// names are. A job whose source came with a name -- a container's file name -- is one
    /// package named after it (`Show.S01.nzb` is `Show.S01`), stated rather than guessed, so the
    /// regroup after the online check leaves it alone. Without one the plugin's package hints
    /// decide, as they always did. A provider's own name for its transfer is no substitute: at
    /// Premiumize it was the upload name `source.nzb` for every NZB (owner report,
    /// 2026-09-27).
    ///
    /// The batch is written first and the row second, and the gap between the two writes is
    /// the one this file does not close: a crash exactly there leaves a row that will poll
    /// `ready` once more and hand the same addresses over a second time. The LinkGrabber's own
    /// duplicate check marks the repeats; a two-phase marker on the row was considered and
    /// not built, because the other order -- close the row, then write the batch -- would on
    /// the same crash lose the addresses for good, and nothing re-polls a finished job.
    pub(super) async fn hand_over(
        &self,
        job: &RemoteJob,
        artifacts: Vec<ReadyArtifact>,
    ) -> anyhow::Result<()> {
        // Read again rather than trusted: the poll that answered `ready` took a while, and a
        // person may have discarded the job in the meantime. The store would refuse the final
        // advance anyway; checking here keeps the batch from being written first.
        let Some(current) = self.inner.database.remote_job(job.id).await? else {
            anyhow::bail!(
                "remote job {} vanished before its addresses were handed over",
                job.id
            );
        };
        if !current.state.may_advance_to(RemoteJobState::Ready) {
            anyhow::bail!(
                "remote job {} is {} and takes no addresses",
                job.id,
                current.state.as_str()
            );
        }
        let count = artifacts.len();
        let runners = self.runners().await?;
        let label = runners
            .plugin_name(&job.plugin_id)
            .map_or_else(|| "remote job".to_owned(), str::to_owned);
        let mut urls = Vec::with_capacity(count);
        let mut file_names = Vec::with_capacity(count);
        let mut sizes = Vec::with_capacity(count);
        let mut package_hints = Vec::with_capacity(count);
        for artifact in artifacts {
            urls.push(artifact.url);
            file_names.push(artifact.file_name);
            sizes.push(
                artifact
                    .size
                    .and_then(|size| rd_core::ByteCount::new(size).ok()),
            );
            package_hints.push(artifact.package_hint);
        }
        let (batch, packages, _) = self
            .inner
            .database
            .add_collector_batch(NewCollectorBatch {
                // No ingress source of its own: routing rules, the review and the blocklist
                // treat what a remote job produced exactly as they treat a pasted link, and
                // the label says where it came from.
                source: IngressSource::Api,
                source_label: Some(label),
                package_name: container_name(&current)
                    .map(rd_collector::container_name)
                    .filter(|name| !name.is_empty()),
                password: None,
                passwords: vec![None; count],
                category_id: None,
                priority: None,
                urls,
                providers: vec![None; count],
                file_names,
                sizes,
                package_hints,
                // A remote job produces one copy of each artifact; there is nothing to
                // mirror it with.
                mirror_hints: Vec::new(),
                requests: vec![None; count],
                body_refs: vec![None; count],
                auto_check: true,
                source_attributes: vec![BTreeMap::new(); count],
            })
            .await?;
        if let Some(link_check) = &self.inner.link_check {
            link_check.check_batch(batch.id).await;
        }
        self.inner
            .database
            .advance_remote_job(
                job.id,
                AdvanceRemoteJob {
                    state: Some(RemoteJobState::Ready),
                    package_id: packages.first().map(|package| package.id),
                    progress_permille: Some(Some(1_000)),
                    code: Some(None),
                    message: Some(None),
                    next_poll_at: Some(None),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(())
    }

    /// A refusal either waits or ends the job; the plugin's category decides which, and the
    /// host's bounds decide how long.
    pub(super) async fn refuse(
        &self,
        job: &RemoteJob,
        refusal: JobRefusal,
        now: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        if !refusal.retryable {
            return self.fail(job, &refusal.code, &refusal.message).await;
        }
        // The row keeps its state and records why it is waiting. For a row still submitting
        // the attempt already counted is what bounds this: after the wait the next step is an
        // adoption check and at most one more submit, never an open-ended retry.
        //
        // A wait the provider named is used as named, clamped. One it did not name doubles
        // the row's previous wait, and that wait is read back off the row as `next_poll_at`
        // minus `updated_at` -- so this write anchors on the clock the store stamps
        // `updated_at` with, not on the tick's `now`, or the next doubling would read the gap
        // between the two clocks instead of the wait.
        let due = match refusal.retry_after_seconds {
            Some(hint) => due_at(now, job.state, Some(hint)),
            None => Utc::now() + seconds(backoff(job)),
        };
        self.inner
            .database
            .advance_remote_job(
                job.id,
                AdvanceRemoteJob {
                    code: Some(Some(refusal.code)),
                    message: Some(Some(clip(&refusal.message))),
                    next_poll_at: Some(Some(due)),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(())
    }

    pub(super) async fn fail(
        &self,
        job: &RemoteJob,
        code: &str,
        message: &str,
    ) -> anyhow::Result<()> {
        self.inner
            .database
            .advance_remote_job(
                job.id,
                AdvanceRemoteJob {
                    state: Some(RemoteJobState::Failed),
                    code: Some(Some(code.to_owned())),
                    message: Some(Some(clip(message))),
                    next_poll_at: Some(None),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(())
    }
}

/// The name a container was added under, and nothing for a magnet or an address.
///
/// Their rows carry a name too since RD-1120-02 -- the `dn`, the last path segment -- but it is
/// there for the list of remote jobs. What reaches the plugin as `job-context.source-name` and
/// what names the LinkGrabber package stays the container's file name alone, as it was.
fn container_name(job: &RemoteJob) -> Option<&str> {
    match job.source_kind {
        RemoteJobSourceKind::Container => job.source_name.as_deref(),
        RemoteJobSourceKind::Magnet | RemoteJobSourceKind::Address => None,
    }
}
