//! Persistent queue scheduler.

mod active;
mod auto_retry;
#[cfg(test)]
mod auto_retry_tests;
mod bandwidth;
mod block;
mod capacity;
mod collision;
#[cfg(test)]
mod collision_tests;
mod content_index;
mod control;
#[cfg(test)]
mod dispatch_tests;
mod enqueue;
mod failures;
mod finish;
mod holds;
mod hostblock;
#[cfg(test)]
mod mirror_fallback_tests;
pub mod mirrors;
mod naming;
mod profile_boundary;
mod provider;
mod queue_pause;
mod rates;
mod replay;
mod retry;
#[cfg(test)]
mod run_guard_tests;
mod runner;
mod worker;

pub use profile_boundary::ProfileBoundary;
pub use worker::{NetworkClient, ProviderCredential};

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use rd_core::{
    AccountId, DownloadId, DownloadState, Failure, FailureKind, PackageId, PluginId, ProxyProfileId,
};
use rd_db::{Database, NewDownload, NewPackage};
use rd_http::{ClientPool, HostLimits, SharedNetworkDefaults};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use url::Url;

use active::ActiveState;
pub use bandwidth::{BandwidthService, BandwidthStatus};
pub use block::BlockReason;
pub use content_index::ContentIndexCheck;
pub use control::NzbDropped;
pub use enqueue::{FileSpec, PackageSpec, ReplaySpec, SecretFragmentSpec};
pub use holds::HoldSource;
use provider::ProviderSlot;
pub use queue_pause::QueuePause;
pub use rates::estimate_seconds;
pub use runner::{ExternalRunner, HTTP_REUSE, RunLimits, RunOutcome};

/// Runtime defaults for queue execution.
#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    pub downloads_directory: PathBuf,
    pub max_active_files: usize,
    pub max_chunks_per_file: usize,
    /// Simultaneous connections one host may see from every transfer together.
    pub max_connections_per_host: usize,
    /// Files an external runner works on at once until the runtime settings say otherwise;
    /// `0` is the runner's own choice (RD-130-22). Here as well as there because the first
    /// dispatch pass runs the moment the scheduler starts.
    pub external_parallel_files: usize,
    pub speed_limit_bytes_per_second: Option<u64>,
    /// Free-space policy shared with every runner and the REST layer.
    pub capacity: rd_files::CapacityService,
    /// Bandwidth profiles, scoped limits and traffic budgets.
    pub bandwidth: bandwidth::BandwidthService,
    /// Proxy, custom CA and TLS generation shared with the HTTP client pool.
    ///
    /// Injectable because a transport that builds its own connections has to share these,
    /// and its service is constructed *before* the scheduler it is registered with.
    pub network_defaults: SharedNetworkDefaults,
    /// Raised by the post-processing service while a package is being processed.
    pub postprocess_hold: rd_core::PostprocessHold,
    /// The address the service's own API listens on. A mirror a Metalink names may never point
    /// at it, even where the person's own network is otherwise allowed (RD-150-03).
    pub own_address: Option<std::net::SocketAddr>,
}

/// Package-level queue attributes chosen at enqueue time.
#[derive(Clone, Copy, Debug, Default)]
pub struct PackageOptions {
    pub category_id: Option<rd_core::CategoryId>,
    pub priority: rd_core::DownloadPriority,
    /// Write the file paused instead of queued, in the same row write, so the dispatcher
    /// cannot start it before a later pause would land (API-09).
    pub paused: bool,
}

/// Settings that can be changed without restarting active transfers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSettings {
    pub max_active_files: usize,
    pub max_chunks_per_file: usize,
    /// Simultaneous connections one host may see across every running transfer; `0` lifts
    /// the limit. Hosters answer an unbounded burst with throttling rather than bytes.
    pub max_connections_per_host: usize,
    /// Connections one external file runner may hold (NNTP); `0` leaves it to the
    /// transport's own server limits (RD-108-25).
    pub external_connections_per_file: usize,
    /// Files an external runner works on at once (NNTP); `0` lets the runner size it by
    /// its own load (RD-130-22), at most [`MAX_EXTERNAL_PARALLEL_FILES`].
    pub external_parallel_files: usize,
    pub speed_limit_bytes_per_second: Option<u64>,
    /// The hand-set upload limit (RD-150-15); like the download one it stays in force through
    /// every profile switch, and the stricter of it and the profile's wins.
    pub upload_limit_bytes_per_second: Option<u64>,
    pub generate_sha256: bool,
    pub global_proxy_profile_id: Option<ProxyProfileId>,
    pub custom_ca_pem: Option<String>,
    /// Retries per file before a retryable failure becomes final.
    pub max_retries: u32,
    /// Hold new downloads while a package is post-processing.
    pub pause_during_postprocess: bool,
    /// Put failed downloads whose failure may pass later back into the queue (RD-191-12).
    pub auto_retry_failed: bool,
    /// Hours between a failure and its automatic retry, [`MIN_AUTO_RETRY_INTERVAL_HOURS`] to
    /// [`MAX_AUTO_RETRY_INTERVAL_HOURS`].
    pub auto_retry_interval_hours: u32,
    /// Automatic retry rounds per download, at most [`MAX_AUTO_RETRY_ROUNDS`]; `0` is no limit.
    pub auto_retry_max_rounds: u32,
    /// Transfer kinds switched off entirely. A queued job of such a kind is blocked with a
    /// reason rather than left waiting, and intake refuses new ones.
    pub disabled_kinds: Vec<rd_core::DownloadKind>,
}

/// Default retries per file.
pub const DEFAULT_MAX_RETRIES: u32 = 8;
/// The most files an external runner may be told to work on at once. Why eight is the
/// runner's to say: `rd_usenet::parallel::MAX_PARALLEL_FILES` carries the reasoning.
pub const MAX_EXTERNAL_PARALLEL_FILES: usize = 8;
pub use auto_retry::{
    DEFAULT_AUTO_RETRY_INTERVAL_HOURS, DEFAULT_AUTO_RETRY_MAX_ROUNDS,
    MAX_AUTO_RETRY_INTERVAL_HOURS, MAX_AUTO_RETRY_ROUNDS, MIN_AUTO_RETRY_INTERVAL_HOURS,
};
/// The per-host connection bounds, re-exported for the callers that configure them. The binary
/// wires the service through this crate and does not link `rd-http` itself.
pub use rd_http::{DEFAULT_CONNECTIONS_PER_HOST, MAX_CONNECTIONS_PER_HOST};
pub use retry::{
    DEFAULT_RATE_LIMIT_WAIT, LIMIT_WAITS_EXHAUSTED_CODE, MAX_CONFIGURABLE_RETRIES, MAX_LIMIT_WAITS,
};

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            max_active_files: 3,
            max_chunks_per_file: 4,
            max_connections_per_host: rd_http::DEFAULT_CONNECTIONS_PER_HOST,
            external_connections_per_file: 0,
            external_parallel_files: 0,
            speed_limit_bytes_per_second: None,
            upload_limit_bytes_per_second: None,
            generate_sha256: true,
            global_proxy_profile_id: None,
            custom_ca_pem: None,
            pause_during_postprocess: true,
            auto_retry_failed: false,
            auto_retry_interval_hours: DEFAULT_AUTO_RETRY_INTERVAL_HOURS,
            auto_retry_max_rounds: DEFAULT_AUTO_RETRY_MAX_ROUNDS,
            disabled_kinds: Vec::new(),
        }
    }
}

impl SchedulerConfig {
    /// Creates local-first defaults for a download directory.
    #[must_use]
    pub fn for_directory(downloads_directory: PathBuf) -> Self {
        Self {
            downloads_directory,
            max_active_files: 3,
            max_chunks_per_file: 4,
            max_connections_per_host: rd_http::DEFAULT_CONNECTIONS_PER_HOST,
            external_parallel_files: 0,
            speed_limit_bytes_per_second: None,
            capacity: rd_files::CapacityService::new(),
            bandwidth: bandwidth::BandwidthService::new(),
            network_defaults: SharedNetworkDefaults::default(),
            postprocess_hold: rd_core::PostprocessHold::new(),
            own_address: None,
        }
    }
}

/// Marks the start's storage recovery as run when it is dropped, panic or not.
struct RecoveryFinished(Arc<tokio::sync::watch::Sender<bool>>);

impl Drop for RecoveryFinished {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StopReason {
    Paused,
    Cancelled,
    /// Stopped by a policy rather than by a person, with the cause that has to survive the
    /// process so the matching release can find it again.
    Blocked(BlockReason),
}

/// Cloneable queue control surface used by REST handlers.
#[derive(Clone)]
pub struct SchedulerHandle {
    database: Database,
    config: Arc<SchedulerConfig>,
    clients: ClientPool,
    resolvers: rd_plugin_host::ResolverService,
    /// Providers that encrypt on the client: the address and how its bytes become a file
    /// (RD-103-02, ADR 0011). Empty on an install with no such plugin, which is every
    /// download that has ever run here so far.
    transforms: Arc<rd_plugin_host::extension::StreamTransformProviders>,
    captcha: rd_captcha::CaptchaBroker,
    secrets: rd_secrets::SecretStore,
    max_active_files: Arc<AtomicUsize>,
    max_chunks_per_file: Arc<AtomicUsize>,
    external_connections_per_file: Arc<AtomicUsize>,
    external_parallel_files: Arc<AtomicUsize>,
    max_retries: Arc<AtomicU32>,
    generate_sha256: Arc<AtomicBool>,
    pause_during_postprocess: Arc<AtomicBool>,
    /// The automatic retry of failed downloads (RD-191-12); see `auto_retry.rs`.
    auto_retry_failed: Arc<AtomicBool>,
    auto_retry_interval_hours: Arc<AtomicU32>,
    auto_retry_max_rounds: Arc<AtomicU32>,
    /// Whether the last pass of the automatic retry found it on; `true` at a start, so the
    /// first pass clears due times a switched-off retry left behind.
    auto_retry_was_enabled: Arc<AtomicBool>,
    /// Transfer kinds the operator switched off. Read on every dispatch so a change takes
    /// effect without a restart.
    disabled_kinds: Arc<Mutex<Vec<rd_core::DownloadKind>>>,
    network_defaults: SharedNetworkDefaults,
    active: Arc<Mutex<ActiveState>>,
    /// Why the network context holds the queue (battery, metered); `None` = running.
    network_hold: Arc<holds::Holds>,
    /// The timed pause of the whole queue (RD-190-20); its record is persisted, this is the copy
    /// the supervise loop checks the end against.
    queue_pause: Arc<Mutex<Option<QueuePause>>>,
    /// Hosters holding back their free downloads after an IP limit.
    host_blocks: hostblock::HostBlocks,
    /// Connections one host may see, and the hosts that proved they ignore ranges.
    host_limits: HostLimits,
    /// A plugin's premium concurrency gate, carrying the limit it is currently sized for so
    /// an upgraded manifest can resize it instead of waiting for a restart.
    provider_slots: Arc<Mutex<HashMap<PluginId, provider::ProviderGate>>>,
    /// One free download per hoster; separate from `provider_slots` so a free download
    /// never consumes a premium account's concurrency (or the other way round).
    free_slots: Arc<Mutex<HashMap<PluginId, Arc<Semaphore>>>>,
    runners: Arc<runner::RunnerRegistry>,
    /// Smoothed transfer rates, sampled by the supervise loop. In memory only.
    rates: Arc<rates::RateSampler>,
    /// One package move at a time. The start carries on unfinished moves in the background
    /// (`recover_storage_work`) while a category change or a completed file can start one for
    /// the same package; two passes of the move protocol at once each found the other's files
    /// already gone (seen as `NotFound` in the scheduler's relocation tests under load).
    relocations: Arc<tokio::sync::Mutex<()>>,
    /// Becomes `true` once the start's background storage work (`recover_storage_work`) has
    /// run, so a caller can tell a move it starts itself from one the start resumed.
    storage_recovered: Arc<tokio::sync::watch::Sender<bool>>,
    shutdown: CancellationToken,
    /// How often a dispatch pass read the startable rows: the tests hold a pass to one read
    /// and a switched-off kind to one more (TR-08, TR-18; re-audit 1.9.1, RA-TR-07).
    #[cfg(test)]
    queue_reads: Arc<AtomicUsize>,
}

impl SchedulerHandle {
    /// Waits until the start's background storage work has run: the moves the previous run
    /// left are carried on and the content index is checked. A move set up before that may be
    /// finished by it, which a test that interrupts a move of its own has to rule out.
    pub async fn storage_recovery_finished(&self) {
        let mut done = self.storage_recovered.subscribe();
        // A sender that is gone has nothing left to wait for.
        let _ = done.wait_for(|finished| *finished).await;
    }

    /// The host capabilities the resolver chain runs on.
    ///
    /// The extension plugin types reach the network through the same host a resolver does,
    /// each narrowed to its own manifest, so this is handed on rather than a second host
    /// being built beside it with its own idea of what is allowed.
    #[must_use]
    pub fn plugin_host(&self) -> Arc<dyn rd_plugin_api::ResolverHost> {
        self.resolvers.host()
    }

    /// Recovers interrupted jobs and starts queue supervision.
    pub async fn start(
        database: Database,
        config: SchedulerConfig,
        secrets: rd_secrets::SecretStore,
        plugins: Option<&rd_plugin_host::PluginTypeRegistry>,
        runners: Vec<Arc<dyn ExternalRunner>>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(&config.downloads_directory)
            .await
            .context("create downloads directory")?;
        database.recover_interrupted().await?;
        let interrupted = database.interrupt_storage_operations().await?;
        if interrupted > 0 {
            tracing::info!(
                interrupted,
                "storage operations of the previous run were interrupted"
            );
        }
        // `session_store::purge_expired` implements a 30-day grace period that had no caller
        // outside its own test, so the `sessions` table grew for the life of the install —
        // silently, because `list_sessions` filters expired rows out anyway. rd-db owns no
        // periodic task of its own, so the sweep runs here, beside the other thing that has to
        // happen once before any work is dispatched.
        let purged = database
            .purge_expired_sessions(database.session_limits().await?)
            .await?;
        if purged > 0 {
            tracing::info!(purged, "removed sessions past their grace period");
        }
        // The same reasoning for the event log: append-only, read by nothing, and without a
        // sweep the largest table in the file on any install that has been running a while.
        let purged_events = database.purge_old_events().await?;
        if purged_events > 0 {
            tracing::info!(purged_events, "removed events past their retention");
        }
        let clients = ClientPool::default();
        let network_defaults = config.network_defaults.clone();
        let captcha = rd_captcha::CaptchaBroker::new(database.clone(), secrets.clone());
        let mut resolvers = rd_plugin_host::ResolverService::new(
            database.clone(),
            clients.clone(),
            secrets.clone(),
            network_defaults.clone(),
            Some(Arc::new(captcha.clone())),
            // The service's own listeners are no plugin's to reach (RA-HOST-01).
            rd_plugin_host::OwnEndpoints::new(config.own_address),
        );
        if let Some(plugins) = plugins {
            let loaded = resolvers.load_components_from_registry(plugins).await?;
            tracing::info!(loaded, "loaded installed resolver components");
        }
        // The twelfth world runs on the same narrowed host a resolver does, so a
        // stream-transform plugin reaches its own manifest's domains and nothing else.
        let transforms = plugins.map_or_else(
            rd_plugin_host::extension::StreamTransformProviders::none,
            |plugins| {
                rd_plugin_host::extension::StreamTransformProviders::from_registry(
                    plugins,
                    Some(resolvers.host()),
                )
            },
        );
        if !transforms.is_empty() {
            tracing::info!(
                loaded = transforms.list().len(),
                "loaded installed stream-transform components"
            );
        }
        // Read before `config` is moved into the handle, and shared by every transfer so
        // the budget belongs to the host rather than to one download.
        let host_limits = HostLimits::new(config.max_connections_per_host);
        let handle = Self {
            database,
            max_active_files: Arc::new(AtomicUsize::new(config.max_active_files)),
            max_retries: Arc::new(AtomicU32::new(DEFAULT_MAX_RETRIES)),
            max_chunks_per_file: Arc::new(AtomicUsize::new(config.max_chunks_per_file)),
            external_connections_per_file: Arc::new(AtomicUsize::new(0)),
            external_parallel_files: Arc::new(AtomicUsize::new(config.external_parallel_files)),
            generate_sha256: Arc::new(AtomicBool::new(true)),
            pause_during_postprocess: Arc::new(AtomicBool::new(true)),
            auto_retry_failed: Arc::new(AtomicBool::new(false)),
            auto_retry_interval_hours: Arc::new(AtomicU32::new(DEFAULT_AUTO_RETRY_INTERVAL_HOURS)),
            auto_retry_max_rounds: Arc::new(AtomicU32::new(DEFAULT_AUTO_RETRY_MAX_ROUNDS)),
            auto_retry_was_enabled: Arc::new(AtomicBool::new(true)),
            disabled_kinds: Arc::new(Mutex::new(Vec::new())),
            network_defaults,
            captcha,
            config: Arc::new(config),
            clients,
            resolvers,
            transforms: Arc::new(transforms),
            secrets,
            active: Arc::new(Mutex::new(ActiveState::default())),
            network_hold: Arc::new(holds::Holds::default()),
            queue_pause: Arc::new(Mutex::new(None)),
            host_blocks: hostblock::HostBlocks::default(),
            host_limits,
            provider_slots: Arc::new(Mutex::new(HashMap::new())),
            free_slots: Arc::new(Mutex::new(HashMap::new())),
            runners: Arc::new(runner::RunnerRegistry::new(runners)),
            rates: Arc::new(rates::RateSampler::default()),
            relocations: Arc::new(tokio::sync::Mutex::new(())),
            storage_recovered: Arc::new(tokio::sync::watch::Sender::new(false)),
            shutdown: CancellationToken::new(),
            #[cfg(test)]
            queue_reads: Arc::new(AtomicUsize::new(0)),
        };
        // Closes `scheduler.before_mirror_promoted`: a group whose active member failed while
        // its successor had not been promoted yet holds nothing and is dispatched by nobody,
        // because the dispatcher only ever looks at rows that are already queued.
        if let Err(error) = handle.recover_stalled_mirror_groups().await {
            tracing::warn!(%error, "stalled mirror groups could not be restarted");
        }
        if let Err(error) = handle.restore_capacity().await {
            tracing::warn!(%error, "storage capacity state could not be restored");
        }
        if let Err(error) = handle.reload_bandwidth().await {
            tracing::warn!(%error, "bandwidth profiles could not be loaded");
        }
        // Before the first dispatch, so a file the pause holds does not start in the gap.
        if let Err(error) = handle.restore_queue_pause().await {
            tracing::warn!(%error, "the timed queue pause could not be restored");
        }
        // Storage work the previous run left: category moves to finish, the history to
        // settle, the content index to check against the disk (RD-150-02). In the background,
        // because a cross-device move is a copy and the queue must not wait for it.
        {
            let recovering = handle.clone();
            tokio::spawn(async move {
                // Sent on the way out however the work ends: a panic in it left every
                // `storage_recovery_finished` waiting for good (audit 1.9.1, T13).
                let _finished = RecoveryFinished(Arc::clone(&recovering.storage_recovered));
                recovering.recover_storage_work().await;
            });
        }
        tokio::spawn(handle.clone().supervise());
        Ok(handle)
    }

    /// Restarts mirror groups that were left with nobody running; see
    /// [`crate::failures::recover_stalled_mirror_groups`].
    async fn recover_stalled_mirror_groups(&self) -> Result<()> {
        crate::failures::recover_stalled_mirror_groups(self).await
    }

    /// Resolver chain (the installed components) for link checks outside the scheduler.
    #[must_use]
    pub fn resolvers(&self) -> rd_plugin_host::ResolverService {
        self.resolvers.clone()
    }

    /// Client for one specific auth profile, used by the profile test action so a
    /// disabled or not-yet-approved profile can still be verified.
    pub async fn test_client(
        &self,
        scope: &Url,
        profile: rd_core::AuthProfile,
    ) -> Result<NetworkClient> {
        worker::build_test_client(self, profile, scope).await
    }

    /// HTTP client honouring the global proxy/TLS defaults without an account identity,
    /// plus any credential headers of the auth profile matching `scope`.
    pub async fn direct_client(&self, scope: &Url) -> Result<NetworkClient> {
        worker::build_client(
            self,
            None,
            None,
            rd_core::AuthProfileSelection::Auto,
            scope,
            None,
        )
        .await
    }

    /// [`Self::direct_client`] held to `policy`: names are resolved through the guard at
    /// connect time and no redirect goes to a refused literal address (RD-150-03). For a
    /// request made on a stranger's word — a link a document or a page proposed.
    pub async fn guarded_client(
        &self,
        scope: &Url,
        policy: rd_http::AddressPolicy,
    ) -> Result<NetworkClient> {
        worker::build_client(
            self,
            None,
            None,
            rd_core::AuthProfileSelection::Auto,
            scope,
            Some(policy),
        )
        .await
    }

    /// The address rule for a request made on a stranger's word (RD-150-03): never this
    /// machine — its loopback and link-local addresses and the address the service listens
    /// on — and the person's own network only when `local_network`.
    #[must_use]
    pub fn remote_address_policy(&self, local_network: bool) -> rd_http::AddressPolicy {
        rd_http::AddressPolicy::new(local_network).listening_on(self.config.own_address)
    }

    /// The address rule a download's source rows were written with: the person's own network
    /// only when every row came from their own hand (`local_network`, decided at intake).
    pub(crate) fn source_address_policy(
        &self,
        sources: &[rd_core::DownloadSource],
    ) -> rd_http::AddressPolicy {
        let local_network =
            !sources.is_empty() && sources.iter().all(|source| source.local_network);
        self.remote_address_policy(local_network)
    }

    /// The rule a queued download's address keeps to, when a stranger's document or page
    /// proposed it or its mirrors (RD-150-03). `None` for a download without source rows,
    /// which is an address the person gave.
    pub(crate) async fn address_policy_for(
        &self,
        id: rd_core::DownloadId,
    ) -> Result<Option<rd_http::AddressPolicy>> {
        let sources = self.database.download_sources(id).await?;
        Ok((!sources.is_empty()).then(|| self.source_address_policy(&sources)))
    }

    /// The captcha broker resolvers hand their challenges to; REST handlers use it to list
    /// and answer the ones waiting for a person.
    #[must_use]
    pub fn captcha(&self) -> rd_captcha::CaptchaBroker {
        self.captcha.clone()
    }

    /// Holds back a hoster's free downloads until `until` after it reported an IP limit.
    pub(crate) fn block_host(&self, source: &url::Url, until: chrono::DateTime<chrono::Utc>) {
        self.host_blocks.block(source, until);
    }

    /// The hosters currently held back by an IP limit, soonest to free up first.
    #[must_use]
    pub fn blocked_hosts(&self) -> Vec<(String, chrono::DateTime<chrono::Utc>)> {
        self.host_blocks.active(chrono::Utc::now())
    }

    /// Forgets every IP limit, because they were tied to an address we no longer have.
    pub fn clear_host_blocks(&self) {
        self.host_blocks.clear();
    }

    /// Puts files that were waiting out an IP limit back in the queue.
    ///
    /// Only those: a file waiting for anything else still has its own reason to wait, and a
    /// reconnect says nothing about a hoster that refused the credentials or a server that
    /// was briefly unreachable.
    pub async fn requeue_ip_blocked(&self) -> anyhow::Result<usize> {
        let waiting = self.database.startable_downloads().await?;
        let mut requeued = 0;
        for file in waiting {
            if file.state != DownloadState::RetryWait {
                continue;
            }
            if !matches!(
                file.last_error.as_ref().map(|failure| &failure.category),
                Some(rd_core::FailureKind::IpBlocked { .. })
            ) {
                continue;
            }
            self.database
                .transition_download(file.id, DownloadState::Queued)
                .await?;
            requeued += 1;
        }
        Ok(requeued)
    }

    /// Default download directory used when no category applies.
    #[must_use]
    pub fn downloads_directory(&self) -> &std::path::Path {
        &self.config.downloads_directory
    }

    /// Applies queue, chunk and bandwidth settings to the running service.
    pub fn validate_runtime_settings(settings: &RuntimeSettings) -> Result<()> {
        anyhow::ensure!(
            settings.max_active_files > 0,
            "max_active_files must be positive"
        );
        anyhow::ensure!(
            settings.max_chunks_per_file > 0,
            "max_chunks_per_file must be positive"
        );
        anyhow::ensure!(
            settings.external_connections_per_file <= 32,
            "external_connections_per_file must be between 0 and 32"
        );
        anyhow::ensure!(
            settings.external_parallel_files <= MAX_EXTERNAL_PARALLEL_FILES,
            "external_parallel_files must be between 0 and {MAX_EXTERNAL_PARALLEL_FILES}"
        );
        anyhow::ensure!(
            settings.max_connections_per_host <= rd_http::MAX_CONNECTIONS_PER_HOST,
            "max_connections_per_host must not exceed {}",
            rd_http::MAX_CONNECTIONS_PER_HOST
        );
        anyhow::ensure!(
            settings.max_retries <= MAX_CONFIGURABLE_RETRIES,
            "max_retries must not exceed {MAX_CONFIGURABLE_RETRIES}"
        );
        anyhow::ensure!(
            (MIN_AUTO_RETRY_INTERVAL_HOURS..=MAX_AUTO_RETRY_INTERVAL_HOURS)
                .contains(&settings.auto_retry_interval_hours),
            "auto_retry_interval_hours must be between {MIN_AUTO_RETRY_INTERVAL_HOURS} and \
             {MAX_AUTO_RETRY_INTERVAL_HOURS}"
        );
        anyhow::ensure!(
            settings.auto_retry_max_rounds <= MAX_AUTO_RETRY_ROUNDS,
            "auto_retry_max_rounds must not exceed {MAX_AUTO_RETRY_ROUNDS}"
        );
        if let Some(certificate) = settings
            .custom_ca_pem
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            reqwest::Certificate::from_pem(certificate.as_bytes())
                .context("invalid custom CA certificate")?;
        }
        Ok(())
    }

    /// Applies validated queue, chunk and network settings to the running service.
    pub async fn update_runtime_settings(&self, settings: RuntimeSettings) -> Result<()> {
        Self::validate_runtime_settings(&settings)?;
        let custom_ca_pem = settings
            .custom_ca_pem
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.into_bytes())
            .into_iter()
            .collect::<Vec<_>>();
        self.max_active_files
            .store(settings.max_active_files, Ordering::Release);
        self.max_chunks_per_file
            .store(settings.max_chunks_per_file, Ordering::Release);
        self.external_connections_per_file
            .store(settings.external_connections_per_file, Ordering::Release);
        self.external_parallel_files
            .store(settings.external_parallel_files, Ordering::Release);
        self.host_limits
            .set_limit(settings.max_connections_per_host);
        self.max_retries
            .store(settings.max_retries, Ordering::Release);
        self.generate_sha256
            .store(settings.generate_sha256, Ordering::Release);
        self.pause_during_postprocess
            .store(settings.pause_during_postprocess, Ordering::Release);
        self.auto_retry_failed
            .store(settings.auto_retry_failed, Ordering::Release);
        self.auto_retry_interval_hours
            .store(settings.auto_retry_interval_hours, Ordering::Release);
        self.auto_retry_max_rounds
            .store(settings.auto_retry_max_rounds, Ordering::Release);
        // Re-enabling a kind has to release what disabling it blocked; otherwise switching a
        // service back on leaves its jobs sitting in `Blocked` with no way to notice.
        let released: Vec<rd_core::DownloadKind> = {
            let mut disabled = self.disabled_kinds.lock().await;
            let released = disabled
                .iter()
                .copied()
                .filter(|kind| !settings.disabled_kinds.contains(kind))
                .collect();
            *disabled = settings.disabled_kinds.clone();
            released
        };
        if !released.is_empty() {
            self.requeue_blocked_of_kinds(&released).await;
        }
        // The hand-set limits are independent of the schedule: they stay in force through
        // every profile switch, and whichever of the two is stricter wins.
        let limits = self.config.bandwidth.limits();
        limits.set_manual_limit(settings.speed_limit_bytes_per_second);
        limits.set_manual_upload_limit(settings.upload_limit_bytes_per_second);
        {
            let mut defaults = self.network_defaults.write().await;
            defaults.global_proxy_profile_id = settings.global_proxy_profile_id;
            defaults.custom_ca_pem = custom_ca_pem;
            defaults.tls_revision = defaults.tls_revision.wrapping_add(1);
        }
        self.clients.clear().await;
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

    /// Requeues what disabling those kinds had blocked — and only that.
    ///
    /// The kind alone is not enough of a filter: a file of a re-enabled kind may also be
    /// blocked because its storage root is full or because its validators changed mid-transfer,
    /// and switching the kind back on is not a verdict on either of those.
    async fn requeue_blocked_of_kinds(&self, kinds: &[rd_core::DownloadKind]) {
        let blocked = match self
            .database
            .downloads_blocked_by(BlockReason::KindDisabled.as_str())
            .await
        {
            Ok(blocked) => blocked,
            Err(error) => {
                tracing::warn!(%error, "jobs blocked by a disabled kind were not read");
                return;
            }
        };
        for id in blocked {
            let file = match self.database.get_download(id).await {
                Ok(Some(file)) => file,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, download = %id, "a blocked job could not be read");
                    continue;
                }
            };
            if !kinds.contains(&file.kind) {
                continue;
            }
            if let Err(error) = self.resume(file.id).await {
                tracing::warn!(%error, download = %file.id, "could not resume a re-enabled kind");
            }
        }
    }

    /// Holds the queue while the machine runs on battery or on a metered connection
    /// (RD-050-13), or while a reconnect is running. Running transfers keep going; only new
    /// starts wait.
    ///
    /// Each source owns its own hold: the power supervisor re-asserts its state every few
    /// seconds, and a single shared slot meant it cleared everybody else's on the way past.
    pub async fn set_network_hold(&self, source: HoldSource, reason: Option<&'static str>) {
        self.network_hold.set(source, reason).await;
    }

    /// Why the queue is currently held, if it is.
    pub async fn network_hold(&self) -> Option<&'static str> {
        self.network_hold.reason().await
    }

    /// The shared proxy/CA defaults, for transports that build their own connections.
    #[must_use]
    pub fn network_defaults(&self) -> SharedNetworkDefaults {
        self.network_defaults.clone()
    }

    /// Builds (or reuses) the HTTP client for a URL, with the auth profile, proxy and
    /// custom CA that would apply to a download of it.
    ///
    /// WebDAV needs this: its `PROPFIND` has to authenticate exactly like the transfer that
    /// follows, and building a second client would bypass the pool and the profile rules.
    pub async fn network_client(
        &self,
        account_id: Option<AccountId>,
        proxy_profile_id: Option<ProxyProfileId>,
        auth_profile: rd_core::AuthProfileSelection,
        scope: &Url,
    ) -> Result<NetworkClient> {
        worker::build_client(
            self,
            account_id,
            proxy_profile_id,
            auth_profile,
            scope,
            None,
        )
        .await
    }

    pub(crate) fn max_retries(&self) -> u32 {
        self.max_retries.load(Ordering::Acquire)
    }

    pub(crate) fn max_chunks_per_file(&self) -> usize {
        self.max_chunks_per_file.load(Ordering::Acquire)
    }

    /// The shared per-host connection policy, handed to every transfer engine.
    pub(crate) fn host_limits(&self) -> &HostLimits {
        &self.host_limits
    }

    /// How many chunks a file from this host may be split into.
    ///
    /// One, once the host has answered a ranged request with something else: the automatic
    /// retry then asks for the whole file in a single connection, which is what pressing
    /// start again used to do by accident.
    pub(crate) fn chunk_budget(&self, url: &Url) -> usize {
        if self.host_limits.ignores_ranges(url) {
            1
        } else {
            self.max_chunks_per_file()
        }
    }

    pub(crate) fn generate_sha256(&self) -> bool {
        self.generate_sha256.load(Ordering::Acquire)
    }

    /// Verifies a configured provider identity through its installed resolver.
    pub async fn check_account(
        &self,
        account_id: AccountId,
    ) -> Result<rd_plugin_host::AccountStatus, Failure> {
        self.resolvers.check_account(account_id).await
    }

    /// Adds a direct URL with an explicit account and/or job proxy selection.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_direct_to_with_network(
        &self,
        source: Url,
        package_name: String,
        file_name: String,
        destination: PathBuf,
        account_id: Option<AccountId>,
        proxy_profile_id: Option<ProxyProfileId>,
        options: PackageOptions,
    ) -> Result<rd_core::DownloadFile> {
        self.enqueue(
            source,
            package_name,
            file_name,
            destination,
            account_id,
            proxy_profile_id,
            options,
        )
        .await
    }

    /// Adds a direct URL to the default directory with explicit network identity.
    pub async fn enqueue_direct_with_network(
        &self,
        source: Url,
        package_name: String,
        file_name: String,
        account_id: Option<AccountId>,
        proxy_profile_id: Option<ProxyProfileId>,
        options: PackageOptions,
    ) -> Result<rd_core::DownloadFile> {
        self.enqueue(
            source,
            package_name,
            file_name,
            self.config.downloads_directory.clone(),
            account_id,
            proxy_profile_id,
            options,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn enqueue(
        &self,
        source: Url,
        package_name: String,
        file_name: String,
        destination: PathBuf,
        account_id: Option<AccountId>,
        proxy_profile_id: Option<ProxyProfileId>,
        options: PackageOptions,
    ) -> Result<rd_core::DownloadFile> {
        let package_id = PackageId::new();
        let destination = rd_files::package_directory(&destination, &package_name);
        self.database
            .create_package(NewPackage {
                id: package_id,
                name: package_name,
                destination: destination.to_string_lossy().into_owned(),
                category_id: options.category_id,
                priority: options.priority,
                postprocess_level: None,
                script: None,
                // A single link added by hand has nothing an enricher looked at.
                enrichment: Vec::new(),
            })
            .await?;
        self.database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id,
                source,
                file_name: rd_files::sanitize_file_name(&file_name),
                total_bytes: None,
                expected_checksum: None,
                account_id,
                proxy_profile_id,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: if options.paused {
                    DownloadState::Paused
                } else {
                    DownloadState::Queued
                },
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                mirror_group: None,
                enrichment: Vec::new(),
                replay: None,
                secret_fragment: None,
            })
            .await
    }

    /// Stops new work, checkpoints active jobs and truncates the WAL.
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown.cancel();
        let tokens = {
            let active = self.active.lock().await;
            active.tokens.values().cloned().collect::<Vec<_>>()
        };
        for token in tokens {
            token.cancel();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            if self.active.lock().await.tokens.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.database.checkpoint_wal().await
    }

    async fn supervise(self) {
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

    async fn schedule_runnable(&self) -> Result<()> {
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
