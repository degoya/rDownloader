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

mod naming;
mod requests;
mod sweep;

pub use naming::source_name;

/// Largest container a remote job may carry: 16 MiB.
///
/// Smaller than the 48 MiB an import takes, on purpose. The bytes are stored in the job's row
/// until the sweep submits them and are then copied into a plugin whose whole memory is 32 to
/// 64 MiB. What a provider takes is smaller still — every `remote-job` plugin shipped today
/// refuses a container over 4 or 8 MiB itself — so this is the host's own ceiling, the torrent
/// intake's 16 MiB, and not a promise that a provider accepts that much.
pub const MAX_REMOTE_JOB_CONTAINER_BYTES: usize = 16 * 1024 * 1024;

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

    /// The provider slugs whose remote-job plugin declares it takes `format` as a container
    /// (`[extension] containers`), read from the installed manifests like [`providers`]
    /// (RD-191-13).
    ///
    /// [`providers`]: RemoteJobService::providers
    pub async fn providers_accepting(
        &self,
        format: &str,
    ) -> anyhow::Result<std::collections::BTreeSet<String>> {
        let manifests = self.inner.plugins.verified_manifests().await?;
        Ok(RemoteJobRunners::providers_accepting(&manifests, format))
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

#[cfg(test)]
#[path = "remote_job_service/package_tests.rs"]
mod package_tests;
