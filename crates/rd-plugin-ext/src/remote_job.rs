//! Remote-job plugins (RD-108-03): the adapter between `world remote-job-plugin` and the
//! sweep that drives a row through it.
//!
//! The host wrapper in `rd_plugin_host::extension::remote_job` already bounds every list and
//! strips every path; what this file adds is the selection and the translation. Which plugin
//! a job runs on is decided twice and by two different keys: a *new* job by the provider slug
//! the account carries, exactly as an authentication flow is; an *existing* job by the plugin
//! id its row names, because the remote identifier and the plugin's own bookkeeping are that
//! plugin's vocabulary and no other plugin's. What a finished job hands back is reduced to
//! addresses the LinkGrabber may take, the same way a crawler's answer is.
//!
//! The wrapper sits behind [`RemoteJobDriver`] so the sweep in `rd-api` and the tests here
//! can be driven without a compiled WebAssembly component. The trait is public because a
//! mock provider in another crate has to implement it; it is hidden because nothing outside a
//! test should.

use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{AccountId, FailureKind, RemoteJobFile};
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry,
    extension::{
        CacheAnswer, CacheKind, CacheQuery, MAX_CACHE_CONTAINER_BYTES, MAX_CACHE_QUERIES,
        RemoteJobHandle, RemoteJobPlugin, RemoteJobProgress, RemoteJobRefusal, RemoteJobSource,
        RemoteJobWork,
    },
};

/// Most addresses taken from one finished job.
///
/// The same figure the crawlers use, and lower than the host wrapper's own ceiling for the
/// same reason: this is the number that lands in somebody's review list.
const MAX_ARTIFACTS: usize = 500;

/// The stable code for a plugin that trapped, ran out of fuel or timed out instead of
/// answering. Not retried: a guest that cannot finish a call says nothing about the job, and
/// a poll loop that waited for it to start finishing would wait for ever.
const UNSPECIFIED: &str = "plugin.remote_job_failed";
/// The stable code for a job that finished with nothing the LinkGrabber could take.
const EMPTY: &str = "plugin.remote_job_empty";
/// The stable code for a row whose plugin is not installed any more.
const NO_PLUGIN: &str = "remote_job.no_plugin";
/// The stable code for a job that asked for a choice again after one was made (RD-120-35).
const CHOICE_NOT_KEPT: &str = "remote_job.choice_not_kept";

/// One address a finished job produced, in the shape the LinkGrabber takes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadyArtifact {
    pub url: url::Url,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    /// The folder the address sat in inside the job; addresses sharing one become one
    /// package.
    pub package_hint: Option<String>,
}

/// Why a call produced no answer, and whether asking again could change that.
///
/// One shape for every refusal the sweep sees, because the sweep asks every refusal the same
/// two questions: what code does the interface translate, and does the row wait or end.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobRefusal {
    /// Stable translation code; never empty.
    pub code: String,
    /// English, redaction-safe text; the fallback when no catalogue carries the code.
    pub message: String,
    /// Whether waiting could plausibly change the answer: an outage, a rate limit, a blocked
    /// address. Everything else ends the job.
    pub retryable: bool,
    /// The wait the provider suggested, when it suggested one. A suggestion: the host clamps
    /// it into its own bounds.
    pub retry_after_seconds: Option<u64>,
}

impl JobRefusal {
    fn permanent(code: &str, message: String) -> Self {
        Self {
            code: code.to_owned(),
            message,
            retryable: false,
            retry_after_seconds: None,
        }
    }

    fn no_plugin(plugin_id: &str) -> Self {
        Self::permanent(
            NO_PLUGIN,
            format!("no installed plugin can run remote job plugin {plugin_id}"),
        )
    }
}

impl From<RemoteJobRefusal> for JobRefusal {
    fn from(refusal: RemoteJobRefusal) -> Self {
        let retry_after_seconds = match refusal.category {
            FailureKind::Transient {
                retry_after_seconds,
            }
            | FailureKind::RateLimited {
                retry_after_seconds,
            }
            | FailureKind::IpBlocked {
                retry_after_seconds,
            } => retry_after_seconds,
            _ => None,
        };
        Self {
            retryable: refusal.is_worth_retrying(),
            retry_after_seconds,
            code: refusal
                .code
                .filter(|code| !code.trim().is_empty())
                .unwrap_or_else(|| UNSPECIFIED.to_owned()),
            message: refusal.message,
        }
    }
}

/// What asking the installed plugins to identify one source produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartOutcome {
    /// The plugin for this provider takes no such source; nothing was written.
    NotClaimed,
    /// The source is one of the plugin's, and this is the key the row is claimed under.
    /// Derived without a request, which is what lets the duplicate guard fire before one.
    Identified {
        plugin_id: String,
        content_key: String,
    },
    /// Nothing can start: no plugin claims the provider, or the plugin refused the source.
    Refused(JobRefusal),
}

/// Where a job stands, as one poll described it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PollOutcome {
    /// Working on something that needs nobody, with the wait the plugin suggested.
    Preparing { retry_after_seconds: Option<u64> },
    /// Nothing moves until a person has chosen. Never empty.
    AwaitingChoice(Vec<RemoteJobFile>),
    /// The provider is fetching.
    Working(RemoteJobWork),
    /// Finished, with the addresses the LinkGrabber may take. Never empty.
    Ready(Vec<ReadyArtifact>),
    /// The provider ended it, the call failed, or the answer held nothing usable.
    Refused(JobRefusal),
}

/// What the sweep needs of one plugin.
///
/// Mirrors the host wrapper call for call. A trait rather than the wrapper itself so the
/// order of writes in the sweep -- the whole idempotency argument -- can be exercised against
/// a mock provider that records what it was asked, without a toolchain in the loop.
#[doc(hidden)]
#[async_trait]
pub trait RemoteJobDriver: Send + Sync {
    async fn claims(&self, source: &RemoteJobSource) -> Result<bool>;
    async fn identify(&self, source: &RemoteJobSource) -> Result<Result<String, RemoteJobRefusal>>;
    async fn submit(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>>;
    async fn adopt(
        &self,
        account: AccountId,
        content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>>;
    async fn poll(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>>;
    async fn choose(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>>;
    async fn discard(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>>;

    /// The kinds of source the provider's cache can be asked about (RD-130-11). The default
    /// is none, which is what every provider without a cache query answers.
    async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
        Ok(Vec::new())
    }

    /// Whether the provider holds each source ready. The default answers `Unknown` for every
    /// query without asking anybody.
    async fn check_cached(
        &self,
        _account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
        Ok(Ok(vec![CacheAnswer::unknown(); queries.len()]))
    }
}

#[async_trait]
impl RemoteJobDriver for RemoteJobPlugin {
    async fn claims(&self, source: &RemoteJobSource) -> Result<bool> {
        Self::claims(self, source).await
    }

    async fn identify(&self, source: &RemoteJobSource) -> Result<Result<String, RemoteJobRefusal>> {
        Self::identify(self, source).await
    }

    async fn submit(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        Self::submit(self, account, source, content_key).await
    }

    async fn adopt(
        &self,
        account: AccountId,
        content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
        Self::adopt(self, account, content_key).await
    }

    async fn poll(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
        Self::poll(self, account, handle).await
    }

    async fn choose(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Self::choose(self, account, handle, chosen).await
    }

    async fn discard(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Self::discard(self, account, handle).await
    }

    async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
        Self::cache_kinds(self).await
    }

    async fn check_cached(
        &self,
        account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
        Self::check_cached(self, account, queries).await
    }
}

/// What one installed plugin is known by, apart from its code.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct RunnerInfo {
    /// The manifest id, stable across versions. What a row names.
    pub plugin_id: String,
    /// The manifest name, for logs and the batch a finished job becomes.
    pub name: String,
    /// The provider slugs whose accounts this plugin runs jobs on, lower-case.
    pub claims: Vec<String>,
}

/// The provider slugs a manifest claims, lower-case: what a runner is routed by and what the
/// form is offered, from one function so the two cannot drift apart.
fn claims_of(manifest: &PluginManifest) -> Vec<String> {
    manifest
        .extension
        .as_ref()
        .map(|extension| {
            extension
                .claims
                .iter()
                .map(|claim| claim.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default()
}

struct Runner {
    info: RunnerInfo,
    plugin: Box<dyn RemoteJobDriver>,
    /// What `cache-kinds` answered, asked once per load (RD-130-11). A plugin that traps on
    /// the question is taken at "none" and never asked again until the next load.
    kinds: tokio::sync::OnceCell<Vec<CacheKind>>,
}

/// The installed remote-job plugins, newest version of each.
pub struct RemoteJobRunners {
    plugins: Vec<Runner>,
    /// Provider slug to the plugin that runs jobs for it. What a *new* job is routed by.
    by_slug: HashMap<String, usize>,
    /// Manifest id to the plugin. What an *existing* row is routed by.
    by_id: HashMap<String, usize>,
}

impl RemoteJobRunners {
    /// Loads every installed remote-job plugin, skipping any that fails to build.
    ///
    /// A broken plugin costs its own feature and nothing else: the service starts, the rows
    /// that name it fail with a code that says so, and the failure is logged.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let loaded = registry.instantiate(&PluginType::RemoteJob, |package| {
            RemoteJobPlugin::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| {
                    (
                        RunnerInfo {
                            plugin_id: package.manifest.id.to_string(),
                            name: package.manifest.name.clone(),
                            claims: claims_of(&package.manifest),
                        },
                        Box::new(plugin) as Box<dyn RemoteJobDriver>,
                    )
                },
            )
        });
        Self::from_drivers(loaded)
    }

    /// Runners over already compiled plugins, for a contract test that built the component
    /// itself.
    #[doc(hidden)]
    #[must_use]
    pub fn from_plugins(plugins: Vec<RemoteJobPlugin>) -> Self {
        Self::from_drivers(
            plugins
                .into_iter()
                .map(|plugin| {
                    let manifest = plugin.manifest();
                    (
                        RunnerInfo {
                            plugin_id: manifest.id.to_string(),
                            name: manifest.name.clone(),
                            claims: claims_of(manifest),
                        },
                        Box::new(plugin) as Box<dyn RemoteJobDriver>,
                    )
                })
                .collect(),
        )
    }

    /// Runners over anything that implements the driver, for a mock provider.
    ///
    /// The input order is the installer's: newest version of each plugin first, so the first
    /// claim of a slug and the first occurrence of an id win.
    #[doc(hidden)]
    #[must_use]
    pub fn from_drivers(loaded: Vec<(RunnerInfo, Box<dyn RemoteJobDriver>)>) -> Self {
        let mut plugins = Vec::new();
        let mut by_slug = HashMap::new();
        let mut by_id = HashMap::new();
        for (info, plugin) in loaded {
            if info.claims.is_empty() {
                // A job runs on an account, and an account belongs to a provider. A plugin
                // that claims none names no account it could run for.
                tracing::warn!(
                    plugin = %info.name,
                    "remote-job plugin claims no provider and cannot be used"
                );
                continue;
            }
            let index = plugins.len();
            for slug in &info.claims {
                // Two plugins claiming one provider is a conflict, not a chain: the first
                // -- the newest -- wins.
                by_slug.entry(slug.clone()).or_insert(index);
            }
            by_id.entry(info.plugin_id.clone()).or_insert(index);
            plugins.push(Runner {
                info,
                plugin,
                kinds: tokio::sync::OnceCell::new(),
            });
        }
        Self {
            plugins,
            by_slug,
            by_id,
        }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
            by_slug: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// The provider slugs jobs can be started on.
    #[must_use]
    pub fn providers(&self) -> BTreeSet<String> {
        self.by_slug.keys().cloned().collect()
    }

    /// The provider slugs the installed `remote-job` manifests claim, without compiling any
    /// of them (RD-120-51).
    ///
    /// The same set [`providers`] answers once the runners are built, read from the manifests
    /// alone: `by_slug` holds every claim of every loaded plugin, lower-cased by the same
    /// [`claims_of`], and a plugin that claims nothing contributes nothing either way. Which
    /// plugin wins a slug two of them claim is the runners' business and does not change the
    /// set. The one difference is a package whose component fails to compile: it is listed
    /// here and skipped there, and a job started on it is refused as `remote_job.no_plugin`.
    ///
    /// [`providers`]: RemoteJobRunners::providers
    #[must_use]
    pub fn claimed_providers<'a>(
        manifests: impl IntoIterator<Item = &'a PluginManifest>,
    ) -> BTreeSet<String> {
        manifests
            .into_iter()
            .filter(|manifest| manifest.plugin_type == PluginType::RemoteJob)
            .flat_map(claims_of)
            .collect()
    }

    /// The name of the plugin a row names, for a log line or a batch label.
    #[must_use]
    pub fn plugin_name(&self, plugin_id: &str) -> Option<&str> {
        self.runner(plugin_id)
            .map(|runner| runner.info.name.as_str())
    }

    /// Whether the plugin for `provider_slug` takes this source, and the key it is known by.
    ///
    /// Reaches nothing. Both calls are answered from the source alone, which is what lets the
    /// caller write the row -- and refuse a duplicate -- before any request goes out.
    pub async fn identify(&self, provider_slug: &str, source: &RemoteJobSource) -> StartOutcome {
        let Some(runner) = self
            .by_slug
            .get(&provider_slug.to_ascii_lowercase())
            .and_then(|index| self.plugins.get(*index))
        else {
            return StartOutcome::Refused(JobRefusal::permanent(
                NO_PLUGIN,
                format!("no installed plugin runs remote jobs on {provider_slug}"),
            ));
        };
        match runner.plugin.claims(source).await {
            Ok(true) => {}
            Ok(false) => return StartOutcome::NotClaimed,
            Err(error) => return StartOutcome::Refused(Self::trapped(runner, &error)),
        }
        match runner.plugin.identify(source).await {
            Ok(Ok(content_key)) => StartOutcome::Identified {
                plugin_id: runner.info.plugin_id.clone(),
                content_key,
            },
            Ok(Err(refusal)) => StartOutcome::Refused(refusal.into()),
            Err(error) => StartOutcome::Refused(Self::trapped(runner, &error)),
        }
    }

    /// Hands the source to the provider through the plugin the row names.
    ///
    /// Assumed not idempotent, and nothing here retries: the attempt ceiling, the adoption
    /// check between attempts and the unique row are all the caller's.
    pub async fn submit(
        &self,
        plugin_id: &str,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
    ) -> Result<RemoteJobHandle, JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(
            runner,
            runner.plugin.submit(account, source, content_key).await,
        )
    }

    /// The job the provider already holds for `content_key`, if any.
    pub async fn adopt(
        &self,
        plugin_id: &str,
        account: AccountId,
        content_key: &str,
    ) -> Result<Option<RemoteJobHandle>, JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(runner, runner.plugin.adopt(account, content_key).await)
    }

    /// Where the job stands now.
    pub async fn poll(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> PollOutcome {
        let Some(runner) = self.runner(plugin_id) else {
            return PollOutcome::Refused(JobRefusal::no_plugin(plugin_id));
        };
        match runner.plugin.poll(account, handle).await {
            Ok(Ok(progress)) => Self::accept(runner, progress),
            Ok(Err(refusal)) => PollOutcome::Refused(refusal.into()),
            Err(error) => PollOutcome::Refused(Self::trapped(runner, &error)),
        }
    }

    /// Where a job stands whose question has already been answered (RD-120-35).
    ///
    /// `choose` returns nothing and nothing the host keeps about the answer reaches the guest
    /// again, so `awaiting-choice` is only honest from a provider that keeps the answer itself
    /// -- Real-Debrid leaves `waiting_files_selection` the moment `selectFiles` succeeds. A
    /// guest that asks again after an answer is one whose provider did not keep it, and
    /// passing the question on would put the row back where it stood before the answer, to
    /// be answered again, for ever. So it ends the job under its own code instead: a row
    /// never moves from an answered question back to an open one.
    pub async fn poll_answered(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> PollOutcome {
        match self.poll(plugin_id, account, handle).await {
            PollOutcome::AwaitingChoice(_) => PollOutcome::Refused(JobRefusal::permanent(
                CHOICE_NOT_KEPT,
                format!(
                    "{} asked for a choice again after it was answered",
                    self.plugin_name(plugin_id)
                        .unwrap_or("the remote-job plugin")
                ),
            )),
            outcome => outcome,
        }
    }

    /// Answers the question the job asked.
    ///
    /// An empty choice never reaches the guest, but that is not decided here: the service
    /// refuses one before it calls this, and the host wrapper refuses one again before it
    /// instantiates anything. This adapter only carries the call through.
    pub async fn choose(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<(), JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(runner, runner.plugin.choose(account, handle, chosen).await)
    }

    /// Removes the job at the provider. Reached from one confirmed request and nothing else.
    pub async fn discard(
        &self,
        plugin_id: &str,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<(), JobRefusal> {
        let runner = self
            .runner(plugin_id)
            .ok_or_else(|| JobRefusal::no_plugin(plugin_id))?;
        Self::settle(runner, runner.plugin.discard(account, handle).await)
    }

    /// Every provider whose plugin can ask a cache, with the kinds it asks about, sorted by
    /// slug (RD-130-11). A provider two plugins claim is the one `by_slug` routes to, the same
    /// rule a new job follows.
    pub async fn cache_providers(&self) -> Vec<(String, Vec<CacheKind>)> {
        let mut slugs: Vec<(&String, &usize)> = self.by_slug.iter().collect();
        slugs.sort_unstable_by(|left, right| left.0.cmp(right.0));
        let mut providers = Vec::new();
        for (slug, index) in slugs {
            let Some(runner) = self.plugins.get(*index) else {
                continue;
            };
            let kinds = Self::kinds_of(runner).await;
            if !kinds.is_empty() {
                providers.push((slug.clone(), kinds.to_vec()));
            }
        }
        providers
    }

    /// Whether the provider behind `provider_slug` holds each source ready (RD-130-11).
    ///
    /// One answer per query, in the order given. A query of a kind the plugin did not name
    /// answers `Unknown` without reaching it; the rest go in batches the host wrapper accepts
    /// -- at most [`MAX_CACHE_QUERIES`] and [`MAX_CACHE_CONTAINER_BYTES`] each -- and the
    /// answers are put back where their queries stood. Any refusal or trap drops the whole
    /// call: a half-answered check is not something the caller could tell from a whole one.
    pub async fn check_cached(
        &self,
        provider_slug: &str,
        account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Vec<CacheAnswer>, JobRefusal> {
        let runner = self
            .by_slug
            .get(&provider_slug.to_ascii_lowercase())
            .and_then(|index| self.plugins.get(*index))
            .ok_or_else(|| {
                JobRefusal::permanent(
                    NO_PLUGIN,
                    format!("no installed plugin asks a cache for {provider_slug}"),
                )
            })?;
        let kinds = Self::kinds_of(runner).await;
        let mut answers = vec![CacheAnswer::unknown(); queries.len()];
        let asked: Vec<usize> = queries
            .iter()
            .enumerate()
            .filter(|(_, query)| kinds.contains(&query.kind))
            .map(|(index, _)| index)
            .collect();
        for batch in cache_batches(queries, &asked) {
            let batch_queries: Vec<CacheQuery> =
                batch.iter().map(|&index| queries[index].clone()).collect();
            let received = Self::settle(
                runner,
                runner.plugin.check_cached(account, &batch_queries).await,
            )?;
            if received.len() != batch.len() {
                // The host wrapper refuses a misaligned list already; a driver that is not
                // the wrapper is held to the same rule here.
                return Err(JobRefusal::permanent(
                    "remote_job.cache_answer_misaligned",
                    format!("{} answered a cache check out of step", runner.info.name),
                ));
            }
            for (index, answer) in batch.into_iter().zip(received) {
                answers[index] = answer;
            }
        }
        Ok(answers)
    }

    /// What `cache-kinds` answered for this runner, asked once.
    async fn kinds_of(runner: &Runner) -> &[CacheKind] {
        runner
            .kinds
            .get_or_init(|| async {
                match runner.plugin.cache_kinds().await {
                    Ok(mut kinds) => {
                        kinds.sort_unstable();
                        kinds.dedup();
                        kinds
                    }
                    Err(error) => {
                        tracing::warn!(
                            plugin = %runner.info.name,
                            %error,
                            "remote-job plugin could not name its cache kinds"
                        );
                        Vec::new()
                    }
                }
            })
            .await
            .as_slice()
    }

    fn runner(&self, plugin_id: &str) -> Option<&Runner> {
        self.by_id
            .get(plugin_id)
            .and_then(|index| self.plugins.get(*index))
    }

    /// Flattens a call's two layers of failure into the one the sweep reads.
    fn settle<T>(
        runner: &Runner,
        answer: Result<Result<T, RemoteJobRefusal>>,
    ) -> Result<T, JobRefusal> {
        match answer {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(refusal)) => Err(refusal.into()),
            Err(error) => Err(Self::trapped(runner, &error)),
        }
    }

    /// A guest that trapped, ran out of fuel or timed out. Logged with the plugin's name and
    /// reported under one code; the message names no provider text because there was none.
    fn trapped(runner: &Runner, error: &anyhow::Error) -> JobRefusal {
        tracing::warn!(plugin = %runner.info.name, %error, "remote-job plugin failed");
        JobRefusal::permanent(
            UNSPECIFIED,
            format!("{} could not complete the call", runner.info.name),
        )
    }

    /// Turns what a plugin said about a job into what the sweep may act on.
    fn accept(runner: &Runner, progress: RemoteJobProgress) -> PollOutcome {
        match progress {
            RemoteJobProgress::Preparing {
                retry_after_seconds,
            } => PollOutcome::Preparing {
                retry_after_seconds,
            },
            RemoteJobProgress::AwaitingChoice { entries } => {
                let entries: Vec<RemoteJobFile> = entries
                    .into_iter()
                    .map(|entry| RemoteJobFile {
                        id: entry.id,
                        path: entry.path,
                        size: entry.size,
                        selected: entry.selected,
                    })
                    .collect();
                if entries.is_empty() {
                    // A question with no options is not a question a person can answer, and
                    // a row parked in `awaiting_choice` with nothing to show would wait for
                    // an answer that cannot be given.
                    return PollOutcome::Refused(JobRefusal::permanent(
                        EMPTY,
                        format!("{} offered nothing to choose from", runner.info.name),
                    ));
                }
                PollOutcome::AwaitingChoice(entries)
            }
            RemoteJobProgress::Working(work) => PollOutcome::Working(work),
            RemoteJobProgress::Ready { artifacts } => {
                let mut accepted = Vec::new();
                for artifact in artifacts.into_iter().take(MAX_ARTIFACTS) {
                    // A proposal that is not an address is dropped rather than reported: the
                    // person who pasted the magnet cannot fix somebody else's plugin.
                    let Ok(url) = url::Url::parse(&artifact.url) else {
                        tracing::warn!(
                            plugin = %runner.info.name,
                            "remote-job plugin proposed something that is not a URL"
                        );
                        continue;
                    };
                    if !matches!(url.scheme(), "http" | "https") {
                        continue;
                    }
                    accepted.push(ReadyArtifact {
                        url,
                        file_name: crate::crawler::sanitize(artifact.file_name.as_deref()),
                        size: artifact.size,
                        package_hint: crate::crawler::sanitize(artifact.package_hint.as_deref()),
                    });
                }
                if accepted.is_empty() {
                    return PollOutcome::Refused(JobRefusal::permanent(
                        EMPTY,
                        format!("{} finished with no address to download", runner.info.name),
                    ));
                }
                PollOutcome::Ready(accepted)
            }
            RemoteJobProgress::Failed(refusal) => PollOutcome::Refused(refusal.into()),
        }
    }
}

/// Splits the positions in `asked` into batches the host wrapper accepts: at most
/// [`MAX_CACHE_QUERIES`] queries and [`MAX_CACHE_CONTAINER_BYTES`] container bytes each. A
/// single container above the byte bound is left out -- it would be refused in any batch --
/// and so answers `Unknown`.
fn cache_batches(queries: &[CacheQuery], asked: &[usize]) -> Vec<Vec<usize>> {
    let mut batches = Vec::new();
    let mut batch: Vec<usize> = Vec::new();
    let mut bytes = 0_usize;
    for &index in asked {
        let weight = match &queries[index].source {
            RemoteJobSource::Container(content) => content.len(),
            RemoteJobSource::Magnet(_) | RemoteJobSource::Address(_) => 0,
        };
        if weight > MAX_CACHE_CONTAINER_BYTES {
            continue;
        }
        if batch.len() == MAX_CACHE_QUERIES || bytes + weight > MAX_CACHE_CONTAINER_BYTES {
            batches.push(std::mem::take(&mut batch));
            bytes = 0;
        }
        batch.push(index);
        bytes += weight;
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use anyhow::Result;
    use async_trait::async_trait;
    use rd_core::{AccountId, FailureKind};
    use rd_plugin_host::extension::{
        CacheAnswer, CacheKind, CacheQuery, CacheState, RemoteJobArtifact, RemoteJobEntry,
        RemoteJobHandle, RemoteJobProgress, RemoteJobRefusal, RemoteJobSource, RemoteJobWork,
    };

    use super::{
        CHOICE_NOT_KEPT, EMPTY, MAX_ARTIFACTS, NO_PLUGIN, PollOutcome, RemoteJobDriver,
        RemoteJobRunners, RunnerInfo, StartOutcome, UNSPECIFIED,
    };

    /// A stand-in plugin: what it claims, what it answers, and the record of what it was
    /// asked.
    struct Fake {
        claims: bool,
        /// The next poll answer; `None` traps.
        progress: Mutex<Option<RemoteJobProgress>>,
        asked: Mutex<Vec<&'static str>>,
    }

    impl Fake {
        fn answering(progress: RemoteJobProgress) -> Self {
            Self {
                claims: true,
                progress: Mutex::new(Some(progress)),
                asked: Mutex::new(Vec::new()),
            }
        }

        fn trapping() -> Self {
            Self {
                claims: true,
                progress: Mutex::new(None),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl RemoteJobDriver for Fake {
        async fn claims(&self, _source: &RemoteJobSource) -> Result<bool> {
            self.asked.lock().expect("asked").push("claims");
            Ok(self.claims)
        }

        async fn identify(
            &self,
            _source: &RemoteJobSource,
        ) -> Result<Result<String, RemoteJobRefusal>> {
            self.asked.lock().expect("asked").push("identify");
            Ok(Ok("c8f1a0b2".to_owned()))
        }

        async fn submit(
            &self,
            _account: AccountId,
            _source: &RemoteJobSource,
            _content_key: &str,
        ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
            self.asked.lock().expect("asked").push("submit");
            anyhow::bail!("out of fuel")
        }

        async fn adopt(
            &self,
            _account: AccountId,
            _content_key: &str,
        ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
            Ok(Ok(None))
        }

        async fn poll(
            &self,
            _account: AccountId,
            _handle: &RemoteJobHandle,
        ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
            self.asked.lock().expect("asked").push("poll");
            match self.progress.lock().expect("progress").take() {
                Some(progress) => Ok(Ok(progress)),
                None => anyhow::bail!("out of fuel"),
            }
        }

        async fn choose(
            &self,
            _account: AccountId,
            _handle: &RemoteJobHandle,
            _chosen: &[u32],
        ) -> Result<Result<(), RemoteJobRefusal>> {
            Ok(Ok(()))
        }

        async fn discard(
            &self,
            _account: AccountId,
            _handle: &RemoteJobHandle,
        ) -> Result<Result<(), RemoteJobRefusal>> {
            Ok(Ok(()))
        }
    }

    const PLUGIN: &str = "019d0000-0000-7000-8000-00000000011d";

    fn build(fake: Fake, claims: &[&str]) -> RemoteJobRunners {
        RemoteJobRunners::from_drivers(vec![(
            RunnerInfo {
                plugin_id: PLUGIN.to_owned(),
                name: "Fake torrents".to_owned(),
                claims: claims.iter().map(|claim| (*claim).to_owned()).collect(),
            },
            Box::new(fake),
        )])
    }

    fn magnet() -> RemoteJobSource {
        RemoteJobSource::Magnet("magnet:?xt=urn:btih:c8f1a0b2".to_owned())
    }

    fn handle() -> RemoteJobHandle {
        RemoteJobHandle {
            remote_id: "XKCD123".to_owned(),
            account_id: AccountId::new().to_string(),
            job_state: None,
        }
    }

    fn artifact(
        url: &str,
        file_name: Option<&str>,
        package_hint: Option<&str>,
    ) -> RemoteJobArtifact {
        RemoteJobArtifact {
            url: url.to_owned(),
            file_name: file_name.map(str::to_owned),
            size: Some(7),
            package_hint: package_hint.map(str::to_owned),
        }
    }

    fn ready(artifacts: Vec<RemoteJobArtifact>) -> RemoteJobProgress {
        RemoteJobProgress::Ready { artifacts }
    }

    /// A new job is routed by the provider slug the account carries; an existing row by the
    /// plugin id it names. Two keys, because they answer two different questions.
    #[tokio::test]
    async fn a_new_job_is_routed_by_provider_and_an_existing_one_by_plugin_id() {
        let runners = build(
            Fake::answering(RemoteJobProgress::Working(RemoteJobWork::default())),
            &["realdebrid"],
        );
        assert_eq!(
            runners.providers().into_iter().collect::<Vec<_>>(),
            vec!["realdebrid".to_owned()]
        );
        // The slug is matched case-insensitively, like everywhere else an account's provider
        // is compared.
        assert_eq!(
            runners.identify("RealDebrid", &magnet()).await,
            StartOutcome::Identified {
                plugin_id: PLUGIN.to_owned(),
                content_key: "c8f1a0b2".to_owned(),
            }
        );
        // A provider nobody claims is a refusal with a code, not a silent nothing.
        assert!(matches!(
            runners.identify("premiumize", &magnet()).await,
            StartOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN && !refusal.retryable
        ));
        // A row naming a plugin that is not installed any more fails the same way.
        assert!(matches!(
            runners.poll("019d0000-0000-7000-8000-0000000000ff", AccountId::new(), &handle()).await,
            PollOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN
        ));
        assert!(matches!(
            runners.poll(PLUGIN, AccountId::new(), &handle()).await,
            PollOutcome::Working(_)
        ));
        assert_eq!(runners.plugin_name(PLUGIN), Some("Fake torrents"));
    }

    /// A source the plugin does not take is not identified either: `identify` is only asked
    /// of something the plugin has said is its own.
    #[tokio::test]
    async fn a_source_the_plugin_does_not_take_is_not_claimed_and_never_identified() {
        let fake = Fake {
            claims: false,
            progress: Mutex::new(None),
            asked: Mutex::new(Vec::new()),
        };
        let runners = build(fake, &["realdebrid"]);
        assert_eq!(
            runners.identify("realdebrid", &magnet()).await,
            StartOutcome::NotClaimed
        );
    }

    /// The offer and the refusal read one table (RD-120-23).
    ///
    /// `providers()` is what the remote-jobs form offers accounts for, and `identify` is
    /// what refuses a provider nobody claims. The two have to be the same table, or the form
    /// offers what the submit rejects -- which is exactly the defect this job was opened
    /// for. Two providers with a plugin and one without, because with a single provider a
    /// correct answer and a wrong one look identical.
    ///
    /// And `remote_job.no_plugin` stays: a plugin can be removed between the form being
    /// drawn and the button being pressed, so the refusal is still the right answer. It
    /// becomes rare, not unnecessary.
    #[tokio::test]
    async fn the_offer_and_the_refusal_read_one_table() {
        let runners = RemoteJobRunners::from_drivers(vec![
            (
                RunnerInfo {
                    plugin_id: PLUGIN.to_owned(),
                    name: "Fake torrents".to_owned(),
                    claims: vec!["realdebrid".to_owned()],
                },
                Box::new(Fake::answering(RemoteJobProgress::Working(
                    RemoteJobWork::default(),
                ))),
            ),
            (
                RunnerInfo {
                    plugin_id: "019d0000-0000-7000-8000-00000000013f".to_owned(),
                    name: "Fake transfers".to_owned(),
                    claims: vec!["premiumize".to_owned()],
                },
                Box::new(Fake::answering(RemoteJobProgress::Working(
                    RemoteJobWork::default(),
                ))),
            ),
        ]);
        assert_eq!(
            runners.providers().into_iter().collect::<Vec<_>>(),
            vec!["premiumize".to_owned(), "realdebrid".to_owned()],
            "both installed plugins are offered, and only they"
        );
        for offered in runners.providers() {
            assert!(
                !matches!(
                    runners.identify(&offered, &magnet()).await,
                    StartOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN
                ),
                "{offered} is offered and refused at once"
            );
        }
        // A service with no plugin is neither offered nor accepted, and the refusal names
        // the code the form's filter exists to make rare.
        assert!(!runners.providers().contains("ddownload"));
        assert!(matches!(
            runners.identify("ddownload", &magnet()).await,
            StartOutcome::Refused(ref refusal) if refusal.code == NO_PLUGIN && !refusal.retryable
        ));
    }

    /// A plugin that claims no provider names no account it could run for, so it is left out
    /// rather than offered for everything.
    #[test]
    fn a_plugin_claiming_no_provider_is_left_out() {
        let runners = build(Fake::trapping(), &[]);
        assert!(runners.is_empty());
        assert!(runners.providers().is_empty());
    }

    /// What a finished job hands back is reduced to addresses the LinkGrabber may take:
    /// http(s) only, names and hints that cannot carry a path out of their folder, and no
    /// more than the review list can hold.
    #[tokio::test]
    async fn only_http_addresses_reach_the_link_grabber() {
        let runners = build(
            Fake::answering(ready(vec![
                artifact(
                    "https://real-debrid.com/d/REDACTED01",
                    Some("ep01.mkv"),
                    Some("Show/Season 1"),
                ),
                artifact("ftp://real-debrid.com/d/REDACTED02", Some("ep02.mkv"), None),
                artifact("not an address", None, None),
                artifact(
                    "http://real-debrid.com/d/REDACTED03",
                    Some("a\\b.mkv"),
                    Some("../escape"),
                ),
            ])),
            &["realdebrid"],
        );
        let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN, AccountId::new(), &handle()).await
        else {
            panic!("expected the addresses");
        };
        assert_eq!(artifacts.len(), 2);
        assert_eq!(
            artifacts[0].url.as_str(),
            "https://real-debrid.com/d/REDACTED01"
        );
        assert_eq!(artifacts[0].file_name.as_deref(), Some("ep01.mkv"));
        assert_eq!(artifacts[0].package_hint.as_deref(), Some("Show/Season 1"));
        assert_eq!(artifacts[0].size, Some(7));
        assert_eq!(artifacts[1].file_name.as_deref(), Some("ab.mkv"));
        assert_eq!(artifacts[1].package_hint.as_deref(), Some("escape"));

        // Finished with nothing usable is a refusal, not an empty package.
        let runners = build(
            Fake::answering(ready(vec![artifact("magnet:?xt=urn:btih:x", None, None)])),
            &["realdebrid"],
        );
        assert!(matches!(
            runners.poll(PLUGIN, AccountId::new(), &handle()).await,
            PollOutcome::Refused(ref refusal) if refusal.code == EMPTY && !refusal.retryable
        ));

        // And the list is bounded, whatever the plugin said.
        let many: Vec<RemoteJobArtifact> = (0..MAX_ARTIFACTS + 100)
            .map(|index| artifact(&format!("https://real-debrid.com/d/{index}"), None, None))
            .collect();
        let runners = build(Fake::answering(ready(many)), &["realdebrid"]);
        let PollOutcome::Ready(artifacts) = runners.poll(PLUGIN, AccountId::new(), &handle()).await
        else {
            panic!("expected the addresses");
        };
        assert_eq!(artifacts.len(), MAX_ARTIFACTS);
    }

    /// A refusal carries the one thing the sweep asks of it -- wait or end -- and the wait the
    /// provider suggested, so the host can clamp it rather than guess.
    #[tokio::test]
    async fn a_refusal_says_whether_waiting_could_change_it() {
        let refused = |category: FailureKind, code: Option<&str>| {
            RemoteJobProgress::Failed(RemoteJobRefusal {
                code: code.map(str::to_owned),
                message: "as the plugin put it".to_owned(),
                category,
            })
        };
        let rate_limited = build(
            Fake::answering(refused(
                FailureKind::RateLimited {
                    retry_after_seconds: Some(120),
                },
                Some("realdebrid_torrents.rate_limited"),
            )),
            &["realdebrid"],
        );
        let PollOutcome::Refused(refusal) =
            rate_limited.poll(PLUGIN, AccountId::new(), &handle()).await
        else {
            panic!("expected a refusal");
        };
        assert!(refusal.retryable);
        assert_eq!(refusal.retry_after_seconds, Some(120));
        assert_eq!(refusal.code, "realdebrid_torrents.rate_limited");
        assert_eq!(refusal.message, "as the plugin put it");

        // The provider ended the job: nothing to wait for.
        let ended = build(
            Fake::answering(refused(
                FailureKind::Permanent,
                Some("realdebrid_torrents.torrent_dead"),
            )),
            &["realdebrid"],
        );
        let PollOutcome::Refused(refusal) = ended.poll(PLUGIN, AccountId::new(), &handle()).await
        else {
            panic!("expected a refusal");
        };
        assert!(!refusal.retryable);
        assert_eq!(refusal.retry_after_seconds, None);

        // A refusal without a code still gets one the interface can translate.
        let unnamed = build(
            Fake::answering(refused(FailureKind::Offline, None)),
            &["realdebrid"],
        );
        let PollOutcome::Refused(refusal) = unnamed.poll(PLUGIN, AccountId::new(), &handle()).await
        else {
            panic!("expected a refusal");
        };
        assert_eq!(refusal.code, UNSPECIFIED);
    }

    /// A guest that trapped, ran out of fuel or timed out says nothing about the job; the
    /// call ends under one code rather than being retried against a plugin that cannot finish.
    #[tokio::test]
    async fn a_plugin_that_traps_ends_the_call_under_one_code() {
        let runners = build(Fake::trapping(), &["realdebrid"]);
        assert!(matches!(
            runners.poll(PLUGIN, AccountId::new(), &handle()).await,
            PollOutcome::Refused(ref refusal) if refusal.code == UNSPECIFIED && !refusal.retryable
        ));
        let refusal = runners
            .submit(PLUGIN, AccountId::new(), &magnet(), "c8f1a0b2")
            .await
            .expect_err("a trap is a refusal");
        assert_eq!(refusal.code, UNSPECIFIED);
        assert!(!refusal.retryable);
    }

    /// A question with nothing to choose from is not a question a person can answer.
    #[tokio::test]
    async fn a_question_with_nothing_to_choose_from_is_refused() {
        let empty = build(
            Fake::answering(RemoteJobProgress::AwaitingChoice {
                entries: Vec::new(),
            }),
            &["realdebrid"],
        );
        assert!(matches!(
            empty.poll(PLUGIN, AccountId::new(), &handle()).await,
            PollOutcome::Refused(ref refusal) if refusal.code == EMPTY
        ));
        let asked = build(
            Fake::answering(RemoteJobProgress::AwaitingChoice {
                entries: vec![RemoteJobEntry {
                    id: 3,
                    path: "Show/ep01.mkv".to_owned(),
                    size: Some(10),
                    selected: true,
                }],
            }),
            &["realdebrid"],
        );
        let PollOutcome::AwaitingChoice(entries) =
            asked.poll(PLUGIN, AccountId::new(), &handle()).await
        else {
            panic!("expected a question");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, 3);
        assert_eq!(entries[0].path, "Show/ep01.mkv");
        assert!(entries[0].selected);
    }

    /// RD-120-35: once a question was answered, asking it again ends the job under its own
    /// code instead of reopening it, and every other answer passes through untouched.
    #[tokio::test]
    async fn a_question_asked_again_after_its_answer_ends_the_job() {
        let asked_again = build(
            Fake::answering(RemoteJobProgress::AwaitingChoice {
                entries: vec![RemoteJobEntry {
                    id: 3,
                    path: "Show/ep01.mkv".to_owned(),
                    size: Some(10),
                    selected: true,
                }],
            }),
            &["realdebrid"],
        );
        let PollOutcome::Refused(refusal) = asked_again
            .poll_answered(PLUGIN, AccountId::new(), &handle())
            .await
        else {
            panic!("an answered question must not be passed on as a new one");
        };
        assert_eq!(refusal.code, CHOICE_NOT_KEPT);
        assert!(
            !refusal.retryable,
            "waiting cannot make the provider remember"
        );

        let moving = build(
            Fake::answering(RemoteJobProgress::Preparing {
                retry_after_seconds: Some(7),
            }),
            &["realdebrid"],
        );
        assert_eq!(
            moving
                .poll_answered(PLUGIN, AccountId::new(), &handle())
                .await,
            PollOutcome::Preparing {
                retry_after_seconds: Some(7)
            }
        );
    }

    /// A stand-in cache (RD-130-11): the kinds it names, how often it was asked for them, and
    /// every batch that reached it. Answers `Cached` with the address as the name, so the
    /// order of the answers can be read back.
    struct CacheFake {
        kinds: Vec<CacheKind>,
        kinds_asked: Mutex<usize>,
        batches: Mutex<Vec<usize>>,
    }

    impl CacheFake {
        fn naming(kinds: &[CacheKind]) -> Self {
            Self {
                kinds: kinds.to_vec(),
                kinds_asked: Mutex::new(0),
                batches: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl RemoteJobDriver for std::sync::Arc<CacheFake> {
        async fn claims(&self, _source: &RemoteJobSource) -> Result<bool> {
            Ok(true)
        }

        async fn identify(
            &self,
            _source: &RemoteJobSource,
        ) -> Result<Result<String, RemoteJobRefusal>> {
            Ok(Ok("c8f1a0b2".to_owned()))
        }

        async fn submit(
            &self,
            _account: AccountId,
            _source: &RemoteJobSource,
            _content_key: &str,
        ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
            anyhow::bail!("a cache check never submits")
        }

        async fn adopt(
            &self,
            _account: AccountId,
            _content_key: &str,
        ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
            Ok(Ok(None))
        }

        async fn poll(
            &self,
            _account: AccountId,
            _handle: &RemoteJobHandle,
        ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
            anyhow::bail!("a cache check never polls")
        }

        async fn choose(
            &self,
            _account: AccountId,
            _handle: &RemoteJobHandle,
            _chosen: &[u32],
        ) -> Result<Result<(), RemoteJobRefusal>> {
            Ok(Ok(()))
        }

        async fn discard(
            &self,
            _account: AccountId,
            _handle: &RemoteJobHandle,
        ) -> Result<Result<(), RemoteJobRefusal>> {
            Ok(Ok(()))
        }

        async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
            *self.kinds_asked.lock().expect("kinds") += 1;
            Ok(self.kinds.clone())
        }

        async fn check_cached(
            &self,
            _account: AccountId,
            queries: &[CacheQuery],
        ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
            self.batches.lock().expect("batches").push(queries.len());
            Ok(Ok(queries
                .iter()
                .map(|query| CacheAnswer {
                    state: CacheState::Cached,
                    file_name: match &query.source {
                        RemoteJobSource::Address(address) | RemoteJobSource::Magnet(address) => {
                            Some(address.clone())
                        }
                        RemoteJobSource::Container(_) => None,
                    },
                    size: None,
                })
                .collect()))
        }
    }

    fn cache_runners(slug: &str, fake: &std::sync::Arc<CacheFake>) -> RemoteJobRunners {
        RemoteJobRunners::from_drivers(vec![(
            RunnerInfo {
                plugin_id: PLUGIN.to_owned(),
                name: "Fake cache".to_owned(),
                claims: vec![slug.to_owned()],
            },
            Box::new(std::sync::Arc::clone(fake)),
        )])
    }

    fn hoster_query(index: usize) -> CacheQuery {
        CacheQuery {
            source: RemoteJobSource::Address(format!("https://hoster.example/f/{index}")),
            kind: CacheKind::Hoster,
        }
    }

    /// RD-130-11: 250 queries reach the plugin as 100, 100 and 50, and every answer comes
    /// back at the position of the query it answers.
    #[tokio::test]
    async fn a_cache_check_is_split_into_batches_and_answered_in_order() {
        let fake = std::sync::Arc::new(CacheFake::naming(&[CacheKind::Hoster]));
        let runners = cache_runners("torbox", &fake);
        let queries: Vec<CacheQuery> = (0..250).map(hoster_query).collect();
        let answers = runners
            .check_cached("TorBox", AccountId::new(), &queries)
            .await
            .expect("answers");
        assert_eq!(*fake.batches.lock().expect("batches"), vec![100, 100, 50]);
        assert_eq!(answers.len(), 250);
        for (index, answer) in answers.iter().enumerate() {
            assert_eq!(
                answer.file_name.as_deref(),
                Some(format!("https://hoster.example/f/{index}").as_str())
            );
        }
    }

    /// A kind the plugin did not name never reaches it, and answers `Unknown` in its place.
    #[tokio::test]
    async fn a_kind_the_plugin_did_not_name_never_reaches_it() {
        let fake = std::sync::Arc::new(CacheFake::naming(&[CacheKind::Torrent]));
        let runners = cache_runners("premiumize", &fake);
        let queries = vec![
            hoster_query(0),
            CacheQuery {
                source: RemoteJobSource::Magnet("magnet:?xt=urn:btih:c8f1a0b2".to_owned()),
                kind: CacheKind::Torrent,
            },
            hoster_query(2),
        ];
        let answers = runners
            .check_cached("premiumize", AccountId::new(), &queries)
            .await
            .expect("answers");
        assert_eq!(*fake.batches.lock().expect("batches"), vec![1]);
        assert_eq!(answers[0], CacheAnswer::unknown());
        assert_eq!(answers[1].state, CacheState::Cached);
        assert_eq!(answers[2], CacheAnswer::unknown());
    }

    /// A plugin with no cache kinds is never asked, is not listed as a cache provider, and is
    /// asked for its kinds once per load however often the check runs.
    #[tokio::test]
    async fn a_plugin_without_cache_kinds_is_never_asked_and_kinds_are_asked_once() {
        let fake = std::sync::Arc::new(CacheFake::naming(&[]));
        let runners = cache_runners("realdebrid", &fake);
        assert!(runners.cache_providers().await.is_empty());
        for _ in 0..3 {
            let answers = runners
                .check_cached("realdebrid", AccountId::new(), &[hoster_query(0)])
                .await
                .expect("answers");
            assert_eq!(answers, vec![CacheAnswer::unknown()]);
        }
        assert!(fake.batches.lock().expect("batches").is_empty());
        assert_eq!(*fake.kinds_asked.lock().expect("kinds"), 1);

        let named = std::sync::Arc::new(CacheFake::naming(&[
            CacheKind::Usenet,
            CacheKind::Torrent,
            CacheKind::Usenet,
        ]));
        let runners = cache_runners("torbox", &named);
        assert_eq!(
            runners.cache_providers().await,
            vec![(
                "torbox".to_owned(),
                vec![CacheKind::Torrent, CacheKind::Usenet]
            )]
        );
        // A provider nobody claims is a refusal with a code, never an empty answer that
        // would read as "asked and not held".
        assert!(matches!(
            runners.check_cached("offcloud", AccountId::new(), &[hoster_query(0)]).await,
            Err(ref refusal) if refusal.code == NO_PLUGIN
        ));
    }
}
