//! Jobs that run at the provider: the sweep that drives a row through its plugin (RD-108-03).
//!
//! RD-107-06 built the contract, the host wrapper, the row and the first plugin, and nothing
//! moved: `due_remote_jobs` read the rows and nobody called it. This is the caller. It lives
//! here rather than in `rd-plugin-ext` because the sweep needs three things only this crate
//! holds together -- the database, the online check and the LinkGrabber's intake -- and the
//! adapter crate has no database dependency and must not grow one.
//!
//! What the loop does is decided elsewhere and only *executed* here: `RemoteJob::submit_step`
//! says whether a row is submitted, adopted, polled or given up on, and
//! `RemoteJobState::poll_delay_seconds` says how long the host waits whatever the plugin
//! suggested. What this file adds is the order of the writes, and that order is the whole
//! idempotency argument of `docs/adr/0003-a-job-that-runs-at-the-provider.md`:
//!
//! 1. the row exists, with its content key, before any request goes out -- [`submit`];
//! 2. an attempt is counted *before* the plugin's `submit` is called, so a crash inside the
//!    call is visible as an attempt and the next tick adopts instead of submitting again;
//! 3. the identifier the provider answers with is written as the very next thing.
//!
//! Nothing is kept in memory between ticks. A restart reads the rows and finds them exactly
//! where the last write left them, which is what makes the restart case a matter of the row's
//! state rather than of luck.
//!
//! [`submit`]: RemoteJobService::submit

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use rd_core::{
    AccountId, IngressSource, RemoteJob, RemoteJobId, RemoteJobSourceKind, RemoteJobState,
    SubmitStep,
};
use rd_db::{AdvanceRemoteJob, ClaimRemoteJob, NewCollectorBatch};
use rd_plugin_ext::{JobRefusal, PollOutcome, ReadyArtifact, RemoteJobRunners, StartOutcome};
use rd_plugin_host::extension::{RemoteJobHandle, RemoteJobSource};
use tokio_util::sync::CancellationToken;

use crate::link_check_service::LinkCheckService;

/// How often due rows are swept. The shortest wait a row can carry is five seconds
/// (`rd_core::MIN_POLL_SECONDS`), so looking more often than that would only find nothing.
const SWEEP: Duration = Duration::from_secs(5);

/// Longest message kept on a row. The text is the plugin's fallback for a code nobody
/// translates; a novel would not help anybody.
const MAX_MESSAGE: usize = 500;

/// Two attempts and an adoption check between them left no identifier. Terminal: a third
/// attempt against an endpoint that is not idempotent says nothing a person could not find out
/// faster by looking at their account.
const SUBMIT_UNCONFIRMED: &str = "remote_job.submit_unconfirmed";
/// The account the row names is gone. The row goes with it in the schema, so this is the one
/// tick between the deletion and the cascade.
const NO_ACCOUNT: &str = "remote_job.no_account";
/// A choice for a job that does not exist.
const NOT_FOUND: &str = "remote_job.not_found";
/// A choice for a job that asked no question.
const NOT_AWAITING_CHOICE: &str = "remote_job.not_awaiting_choice";
/// A choice naming nothing the job offered. Refused here and again in the host wrapper: at one
/// provider it is an error and at another it silently means "all of them".
const EMPTY_CHOICE: &str = "remote_job.empty_choice";
/// A job in a state that needs a remote identifier and has none. Cannot happen through this
/// file's own writes; named rather than unwrapped.
const MISSING_REMOTE_ID: &str = "remote_job.missing_remote_id";
/// A request to delete at the provider that did not say so. The one refusal in this file that
/// is not about the row at all: deleting at somebody else's provider is irreversible and
/// outside this machine, so the confirmation is a value the request has to carry rather than
/// something a client is trusted to have asked for (ADR 0003).
const NOT_CONFIRMED: &str = "remote_job.not_confirmed";
/// A second confirmed deletion of a job that is already gone from the provider.
const ALREADY_DISCARDED: &str = "remote_job.already_discarded";
/// What a row records once a confirmed request removed the job at the provider. The row stays,
/// with its remote identifier, so the account that lost a torrent can be shown which request
/// removed it and when.
const DISCARDED: &str = "remote_job.discarded";

/// Why a request was refused, in the shape a handler translates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteJobRefused {
    pub code: String,
    pub message: String,
}

impl RemoteJobRefused {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

impl From<JobRefusal> for RemoteJobRefused {
    fn from(refusal: JobRefusal) -> Self {
        Self {
            code: refusal.code,
            message: refusal.message,
        }
    }
}

/// What handing a source to an account produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmitOutcome {
    /// A row was written; the sweep takes it from here.
    Started(RemoteJob),
    /// The account already has a job for this content. Nothing was written and nothing was
    /// sent: this is the duplicate guard, and it fires before the first network call.
    AlreadyOurs(RemoteJob),
    /// The plugin for this provider takes no such source. Nothing was written.
    NotClaimed,
    /// Nothing could start, with the code that says why.
    Refused(RemoteJobRefused),
}

/// What answering a job's question produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChoiceOutcome {
    /// The provider took the choice; the row is polled again from now. Boxed because the
    /// refusal beside it is small and the row is not.
    Chosen(Box<RemoteJob>),
    /// The choice was refused, and the row still waits for one.
    Refused(RemoteJobRefused),
}

/// What a confirmed request to delete at the provider produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscardOutcome {
    /// The provider removed it. The row stays, in `discarded`, as the record that it did.
    Discarded(Box<RemoteJob>),
    /// Nothing was sent, with the code that says why.
    Refused(RemoteJobRefused),
}

struct Inner {
    database: rd_db::Database,
    plugins: rd_plugin_host::PluginInstaller,
    plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
    /// `None` only in a test that drives the sweep by hand. The online check is a service
    /// with a scheduler behind it; a batch handed over without one starts its links in
    /// `checking` and leaves them there, which is what such a test then asserts on.
    link_check: Option<LinkCheckService>,
    runners: tokio::sync::OnceCell<Arc<RemoteJobRunners>>,
    /// What `load` answers instead of reading the plugin directory, in order. Empty in
    /// production; a test scripts a failed load followed by a good one.
    #[cfg(test)]
    scripted: std::sync::Mutex<std::collections::VecDeque<Result<RemoteJobRunners, String>>>,
    shutdown: CancellationToken,
}

/// Cloneable handle of the remote job service.
#[derive(Clone)]
pub struct RemoteJobService {
    inner: Arc<Inner>,
}

impl RemoteJobService {
    #[must_use]
    pub fn start(
        database: rd_db::Database,
        plugins: rd_plugin_host::PluginInstaller,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
        link_check: LinkCheckService,
    ) -> Self {
        let service = Self {
            inner: Arc::new(Inner {
                database,
                plugins,
                plugin_host,
                link_check: Some(link_check),
                runners: tokio::sync::OnceCell::new(),
                #[cfg(test)]
                scripted: std::sync::Mutex::new(std::collections::VecDeque::new()),
                shutdown: CancellationToken::new(),
            }),
        };
        tokio::spawn(service.clone().sweep_loop());
        service
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// A service with no sweep loop and the given plugins, for tests that drive one sweep by
    /// hand against a mock provider.
    #[cfg(test)]
    fn detached(
        database: rd_db::Database,
        plugin_root: std::path::PathBuf,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
        runners: RemoteJobRunners,
    ) -> Self {
        Self::detached_scripted(database, plugin_root, plugin_host, vec![Ok(runners)])
    }

    /// Like `detached`, with every load scripted: each entry is what one attempt to load the
    /// plugins answers, so a test can make the first tick fail and the second succeed.
    #[cfg(test)]
    fn detached_scripted(
        database: rd_db::Database,
        plugin_root: std::path::PathBuf,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
        loads: Vec<Result<RemoteJobRunners, String>>,
    ) -> Self {
        Self::detached_over(
            database,
            rd_plugin_host::PluginInstaller::new(
                plugin_root,
                rd_plugin_host::PluginVerifier::new(false),
            ),
            plugin_host,
            loads,
        )
    }

    /// Like `detached_scripted`, over an installer the test built -- one that accepts an
    /// unsigned package, for a test about what is read from an installed manifest.
    #[cfg(test)]
    fn detached_over(
        database: rd_db::Database,
        plugins: rd_plugin_host::PluginInstaller,
        plugin_host: Arc<dyn rd_plugin_api::ResolverHost>,
        loads: Vec<Result<RemoteJobRunners, String>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                database,
                plugins,
                plugin_host,
                link_check: None,
                runners: tokio::sync::OnceCell::new(),
                scripted: std::sync::Mutex::new(loads.into_iter().collect()),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    /// The installed remote-job plugins, compiled on first use.
    ///
    /// Lazily, like the authentication providers: compiling every component at startup would
    /// delay the service for a feature most installations never touch, and a plugin that
    /// fails to build costs its own feature and nothing else.
    ///
    /// A failure to *load* -- a disk error, not a missing plugin -- leaves the cell empty and
    /// is handed to the caller: the tick that hit it is skipped and the next one tries again.
    /// Remembering it as "no plugins" would end every running job under a terminal code for a
    /// cause that may be gone in a second.
    pub async fn runners(&self) -> anyhow::Result<Arc<RemoteJobRunners>> {
        let runners = self
            .inner
            .runners
            .get_or_try_init(|| async { self.load().await.map(Arc::new) })
            .await?;
        Ok(Arc::clone(runners))
    }

    /// The provider slugs a job can be started on, read from the installed manifests.
    ///
    /// Deliberately not [`runners`]: that compiles every installed component on its first
    /// call, and the form that asks this question is drawn the moment the remote-job page
    /// opens -- the first open after a start used to wait for all of it, behind an empty
    /// account picker (RD-120-51). The manifests are signature-checked exactly as a load
    /// checks them; only the compile is left out, and the answer is the same set
    /// ([`RemoteJobRunners::claimed_providers`] says where it could differ).
    ///
    /// [`runners`]: RemoteJobService::runners
    pub async fn providers(&self) -> anyhow::Result<std::collections::BTreeSet<String>> {
        let manifests = self.inner.plugins.verified_manifests().await?;
        Ok(RemoteJobRunners::claimed_providers(&manifests))
    }

    async fn load(&self) -> anyhow::Result<RemoteJobRunners> {
        #[cfg(test)]
        if let Some(scripted) = self
            .inner
            .scripted
            .lock()
            .expect("scripted loads")
            .pop_front()
        {
            return scripted.map_err(|message| anyhow::anyhow!(message));
        }
        let runners = RemoteJobRunners::load(
            &self.inner.plugins,
            Some(Arc::clone(&self.inner.plugin_host)),
        )
        .await?;
        if !runners.is_empty() {
            tracing::info!("remote-job plugins loaded");
        }
        Ok(runners)
    }

    /// Hands a source to an account: the entry point of every remote job.
    ///
    /// Everything before the row is written happens without a request. The plugin is asked
    /// whether it takes the source and what key it is known by, both answered locally; the
    /// key is looked up and then claimed under the unique index; and only a row that was
    /// actually written is ever offered to the sweep, which is the only place the plugin's
    /// `submit` is called. A second paste of the same magnet therefore ends in
    /// [`SubmitOutcome::AlreadyOurs`] before anything has left the machine.
    pub async fn submit(
        &self,
        account_id: AccountId,
        source: RemoteJobSource,
    ) -> anyhow::Result<SubmitOutcome> {
        let accounts = self.inner.database.list_accounts().await?;
        let Some(account) = accounts
            .into_iter()
            .find(|account| account.id == account_id)
        else {
            return Ok(SubmitOutcome::Refused(RemoteJobRefused::new(
                NO_ACCOUNT,
                "the account this job would run on does not exist",
            )));
        };
        let runners = self.runners().await?;
        let (plugin_id, content_key) = match runners.identify(&account.provider, &source).await {
            StartOutcome::NotClaimed => return Ok(SubmitOutcome::NotClaimed),
            StartOutcome::Refused(refusal) => return Ok(SubmitOutcome::Refused(refusal.into())),
            StartOutcome::Identified {
                plugin_id,
                content_key,
            } => (plugin_id, content_key),
        };
        if let Some(existing) = self
            .inner
            .database
            .remote_job_by_content(account_id, &content_key)
            .await?
        {
            return Ok(SubmitOutcome::AlreadyOurs(existing));
        }
        let (source_kind, bytes) = match source {
            RemoteJobSource::Magnet(address) => (RemoteJobSourceKind::Magnet, address.into_bytes()),
            RemoteJobSource::Container(bytes) => (RemoteJobSourceKind::Container, bytes),
            RemoteJobSource::Address(address) => {
                (RemoteJobSourceKind::Address, address.into_bytes())
            }
        };
        let claim = ClaimRemoteJob {
            id: RemoteJobId::new(),
            account_id,
            plugin_id,
            content_key: content_key.clone(),
            source_kind,
            source: bytes,
            package_id: None,
        };
        match self.inner.database.claim_remote_job(claim).await {
            Ok(job) => Ok(SubmitOutcome::Started(job)),
            Err(error) => {
                // Two submits in the same moment: the read above saw nothing and the unique
                // index refused the second insert. That is the guard doing its job, not a
                // failure, so the row the other submit wrote is what this one answers with.
                match self
                    .inner
                    .database
                    .remote_job_by_content(account_id, &content_key)
                    .await?
                {
                    Some(existing) => Ok(SubmitOutcome::AlreadyOurs(existing)),
                    None => Err(error),
                }
            }
        }
    }

    /// Answers the question a job in `awaiting_choice` asked.
    ///
    /// The choice is kept to what the job offered, an empty one never reaches the guest, and
    /// a refusal leaves the row waiting: the question stays open until it is answered.
    pub async fn choose(
        &self,
        id: RemoteJobId,
        requested: &[u32],
    ) -> anyhow::Result<ChoiceOutcome> {
        let Some(job) = self.inner.database.remote_job(id).await? else {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                NOT_FOUND,
                "there is no such remote job",
            )));
        };
        if job.state != RemoteJobState::AwaitingChoice {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                NOT_AWAITING_CHOICE,
                "this remote job is not waiting for a choice",
            )));
        }
        let chosen = job.accept_choice(requested);
        if chosen.is_empty() {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                EMPTY_CHOICE,
                "a selection has to name at least one entry the job offered",
            )));
        }
        let Some(handle) = handle_of(&job) else {
            return Ok(ChoiceOutcome::Refused(RemoteJobRefused::new(
                MISSING_REMOTE_ID,
                "this remote job names no job at the provider",
            )));
        };
        let runners = self.runners().await?;
        if let Err(refusal) = runners
            .choose(&job.plugin_id, job.account_id, &handle, &chosen)
            .await
        {
            return Ok(ChoiceOutcome::Refused(refusal.into()));
        }
        let job = self
            .inner
            .database
            .advance_remote_job(
                id,
                AdvanceRemoteJob {
                    state: Some(RemoteJobState::Working),
                    chosen: Some(chosen),
                    code: Some(None),
                    message: Some(None),
                    // Polled again at once: the person is waiting to see it move.
                    next_poll_at: Some(Some(Utc::now())),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(ChoiceOutcome::Chosen(Box::new(job)))
    }

    /// Every remote job this installation knows about, newest first.
    pub async fn jobs(&self) -> anyhow::Result<Vec<RemoteJob>> {
        self.inner.database.all_remote_jobs().await
    }

    /// Deletes the job **at the provider**, on one explicit confirmed request.
    ///
    /// This is the only path in the application that reaches a plugin's `discard`, which is
    /// what ADR 0003 asks for: not from removing the local package, not from a failed job, not
    /// from a cleanup sweep, and not from a request that merely arrived at this endpoint. The
    /// confirmation is a value the caller has to carry, so "was it confirmed" is answerable
    /// from the request rather than from a client's good intentions.
    ///
    /// What it did is recorded rather than erased. The row stays, in `discarded`, still naming
    /// the job the provider knew — an account that lost a torrent can be shown which request
    /// removed it and when. Forgetting the row is a separate act ([`forget`]).
    ///
    /// [`forget`]: RemoteJobService::forget
    pub async fn discard(
        &self,
        id: RemoteJobId,
        confirmed: bool,
    ) -> anyhow::Result<DiscardOutcome> {
        if !confirmed {
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                NOT_CONFIRMED,
                "deleting a job at the provider has to be confirmed explicitly",
            )));
        }
        let Some(job) = self.inner.database.remote_job(id).await? else {
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                NOT_FOUND,
                "there is no such remote job",
            )));
        };
        if job.state == RemoteJobState::Discarded {
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                ALREADY_DISCARDED,
                "this remote job has already been removed at the provider",
            )));
        }
        let Some(handle) = handle_of(&job) else {
            // Nothing was ever created there, so there is nothing to delete. Saying so is more
            // use than sending a request that would name no job: the row itself can be removed
            // from the list, which touches no provider.
            return Ok(DiscardOutcome::Refused(RemoteJobRefused::new(
                MISSING_REMOTE_ID,
                "this remote job names no job at the provider",
            )));
        };
        let runners = self.runners().await?;
        if let Err(refusal) = runners
            .discard(&job.plugin_id, job.account_id, &handle)
            .await
        {
            return Ok(DiscardOutcome::Refused(refusal.into()));
        }
        tracing::info!(
            remote_job = %job.id,
            account = %job.account_id,
            plugin = %job.plugin_id,
            remote_id = %handle.remote_id,
            "a confirmed request removed a job at the provider"
        );
        let job = self
            .inner
            .database
            .advance_remote_job(
                id,
                AdvanceRemoteJob {
                    state: Some(RemoteJobState::Discarded),
                    code: Some(Some(DISCARDED.to_owned())),
                    message: Some(Some(clip("removed at the provider on a confirmed request"))),
                    next_poll_at: Some(None),
                    ..AdvanceRemoteJob::default()
                },
            )
            .await?;
        Ok(DiscardOutcome::Discarded(Box::new(job)))
    }

    /// Removes the row from this installation's list, and nothing else.
    ///
    /// The counterpart of [`discard`], and the reason the two are separate calls: what the
    /// provider holds is untouched, so a person clearing a finished job out of a list cannot
    /// accidentally delete a torrent in their account. Answers whether a row was there.
    ///
    /// [`discard`]: RemoteJobService::discard
    pub async fn forget(&self, id: RemoteJobId) -> anyhow::Result<bool> {
        self.inner.database.delete_remote_job(id).await
    }

    async fn sweep_loop(self) {
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
    async fn drive(
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
    async fn submit_row(
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
            .submit(&job.plugin_id, job.account_id, &source, &job.content_key)
            .await
        {
            Ok(handle) => self.named(job, handle, false, now).await,
            Err(refusal) => self.refuse(job, refusal, now).await,
        }
    }

    /// Asks the provider what it already holds for this content, before anything is sent a
    /// second time.
    async fn adopt_row(
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
    async fn named(
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
    async fn settle(
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
    /// The batch is written first and the row second, and the gap between the two writes is
    /// the one this file does not close: a crash exactly there leaves a row that will poll
    /// `ready` once more and hand the same addresses over a second time. The LinkGrabber's own
    /// duplicate check marks the repeats; a two-phase marker on the row was considered and
    /// not built, because the other order -- close the row, then write the batch -- would on
    /// the same crash lose the addresses for good, and nothing re-polls a finished job.
    async fn hand_over(
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
                package_name: None,
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
    async fn refuse(
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

    async fn fail(&self, job: &RemoteJob, code: &str, message: &str) -> anyhow::Result<()> {
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

/// The handle a row stands for, or `None` while the provider has not named the job.
fn handle_of(job: &RemoteJob) -> Option<RemoteJobHandle> {
    Some(RemoteJobHandle {
        remote_id: job.remote_id.clone()?,
        account_id: job.account_id.to_string(),
        job_state: job.job_state.clone(),
    })
}

/// When a row in `state` is next due, given what the plugin suggested.
///
/// The host owns the clock: the suggestion is clamped by `poll_delay_seconds`, and a state
/// that is not polled at all -- which cannot reach here, but is named -- waits the maximum.
fn due_at(now: DateTime<Utc>, state: RemoteJobState, hint: Option<u64>) -> DateTime<Utc> {
    now + seconds(
        state
            .poll_delay_seconds(hint)
            .unwrap_or(rd_core::MAX_POLL_SECONDS),
    )
}

/// `rd_core::MAX_POLL_SECONDS` as chrono wants it; asserted equal so the two cannot drift.
const MAX_POLL: i64 = 900;
const _: () = assert!(MAX_POLL.unsigned_abs() == rd_core::MAX_POLL_SECONDS);

/// A wait in the host's bounds, as a duration. Never longer than the maximum, whatever the
/// caller computed.
fn seconds(value: u64) -> chrono::Duration {
    chrono::Duration::seconds(
        i64::try_from(value.min(rd_core::MAX_POLL_SECONDS)).unwrap_or(MAX_POLL),
    )
}

/// The next wait for a refusal that named none: twice the row's previous wait, from the
/// state's own default up to `MAX_POLL_SECONDS`.
///
/// The previous wait is read off the row -- `next_poll_at` minus `updated_at`, exactly what
/// the last write set -- and counts only when that write was a refusal, which `code` records.
/// A successful answer clears the code and so resets the doubling. Without this a provider
/// that is down for a day would be asked every fifteen to thirty seconds per row, because the
/// plugin maps a 5xx to a transient refusal with no wait of its own.
fn backoff(job: &RemoteJob) -> u64 {
    let default = job
        .state
        .poll_delay_seconds(None)
        .unwrap_or(rd_core::MAX_POLL_SECONDS);
    let previous = match (&job.code, job.next_poll_at) {
        // Rounded, not truncated: the store stamps `updated_at` a few milliseconds after
        // the due time was computed, and truncation would turn every thirty into a
        // twenty-nine and the ladder into 29, 58, 116.
        (Some(_), Some(due)) => {
            u64::try_from(((due - job.updated_at).num_milliseconds() + 500) / 1_000)
                .unwrap_or(default)
                .max(default)
        }
        _ => default,
    };
    previous.saturating_mul(2).min(rd_core::MAX_POLL_SECONDS)
}

fn clip(message: &str) -> String {
    message.chars().take(MAX_MESSAGE).collect()
}

#[cfg(test)]
#[path = "remote_job_service/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "remote_job_service/provider_tests.rs"]
mod provider_tests;
