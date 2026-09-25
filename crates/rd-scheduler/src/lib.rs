//! Persistent queue scheduler.

mod bandwidth;
mod block;
mod capacity;
mod control;
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
mod rates;
mod replay;
mod retry;
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

pub use bandwidth::{BandwidthService, BandwidthStatus};
pub use block::BlockReason;
pub use enqueue::{FileSpec, PackageSpec, ReplaySpec, SecretFragmentSpec};
pub use holds::HoldSource;
use provider::ProviderSlot;
pub use rates::estimate_seconds;
pub use runner::{ExternalRunner, RunLimits, RunOutcome};

/// Runtime defaults for queue execution.
#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    pub downloads_directory: PathBuf,
    pub max_active_files: usize,
    pub max_chunks_per_file: usize,
    /// Simultaneous connections one host may see from every transfer together.
    pub max_connections_per_host: usize,
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
}

/// Package-level queue attributes chosen at enqueue time.
#[derive(Clone, Copy, Debug, Default)]
pub struct PackageOptions {
    pub category_id: Option<rd_core::CategoryId>,
    pub priority: rd_core::DownloadPriority,
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
    pub speed_limit_bytes_per_second: Option<u64>,
    pub generate_sha256: bool,
    pub global_proxy_profile_id: Option<ProxyProfileId>,
    pub custom_ca_pem: Option<String>,
    /// Retries per file before a retryable failure becomes final.
    pub max_retries: u32,
    /// Hold new downloads while a package is post-processing.
    pub pause_during_postprocess: bool,
    /// Transfer kinds switched off entirely. A queued job of such a kind is blocked with a
    /// reason rather than left waiting, and intake refuses new ones.
    pub disabled_kinds: Vec<rd_core::DownloadKind>,
}

/// Default retries per file.
pub const DEFAULT_MAX_RETRIES: u32 = 8;
/// The per-host connection bounds, re-exported for the callers that configure them. The binary
/// wires the service through this crate and does not link `rd-http` itself.
pub use rd_http::{DEFAULT_CONNECTIONS_PER_HOST, MAX_CONNECTIONS_PER_HOST};
pub use retry::MAX_CONFIGURABLE_RETRIES;

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            max_active_files: 3,
            max_chunks_per_file: 4,
            max_connections_per_host: rd_http::DEFAULT_CONNECTIONS_PER_HOST,
            external_connections_per_file: 0,
            speed_limit_bytes_per_second: None,
            generate_sha256: true,
            global_proxy_profile_id: None,
            custom_ca_pem: None,
            pause_during_postprocess: true,
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
            speed_limit_bytes_per_second: None,
            capacity: rd_files::CapacityService::new(),
            bandwidth: bandwidth::BandwidthService::new(),
            network_defaults: SharedNetworkDefaults::default(),
            postprocess_hold: rd_core::PostprocessHold::new(),
        }
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

#[derive(Default)]
struct ActiveState {
    tokens: HashMap<DownloadId, CancellationToken>,
    reasons: HashMap<DownloadId, StopReason>,
    /// Running files whose runner does not count against `max_active_files`.
    exempt: std::collections::HashSet<DownloadId>,
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
    max_retries: Arc<AtomicU32>,
    generate_sha256: Arc<AtomicBool>,
    pause_during_postprocess: Arc<AtomicBool>,
    /// Transfer kinds the operator switched off. Read on every dispatch so a change takes
    /// effect without a restart.
    disabled_kinds: Arc<Mutex<Vec<rd_core::DownloadKind>>>,
    network_defaults: SharedNetworkDefaults,
    active: Arc<Mutex<ActiveState>>,
    /// Why the network context holds the queue (battery, metered); `None` = running.
    network_hold: Arc<holds::Holds>,
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
    shutdown: CancellationToken,
}

impl SchedulerHandle {
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
            generate_sha256: Arc::new(AtomicBool::new(true)),
            pause_during_postprocess: Arc::new(AtomicBool::new(true)),
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
            host_blocks: hostblock::HostBlocks::default(),
            host_limits,
            provider_slots: Arc::new(Mutex::new(HashMap::new())),
            free_slots: Arc::new(Mutex::new(HashMap::new())),
            runners: Arc::new(runner::RunnerRegistry::new(runners)),
            rates: Arc::new(rates::RateSampler::default()),
            shutdown: CancellationToken::new(),
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
        tokio::spawn(handle.clone().supervise());
        Ok(handle)
    }

    /// Restarts mirror groups that were left with nobody running; see
    /// [`crate::failures::recover_stalled_mirror_groups`].
    async fn recover_stalled_mirror_groups(&self) -> Result<()> {
        crate::failures::recover_stalled_mirror_groups(self).await
    }

    /// Resolver chain (native + installed components) for link checks outside the scheduler.
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
        worker::build_client(self, None, None, rd_core::AuthProfileSelection::Auto, scope).await
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
        let waiting = self.database.list_downloads().await?;
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
            settings.max_connections_per_host <= rd_http::MAX_CONNECTIONS_PER_HOST,
            "max_connections_per_host must not exceed {}",
            rd_http::MAX_CONNECTIONS_PER_HOST
        );
        anyhow::ensure!(
            settings.max_retries <= MAX_CONFIGURABLE_RETRIES,
            "max_retries must not exceed {MAX_CONFIGURABLE_RETRIES}"
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
        self.host_limits
            .set_limit(settings.max_connections_per_host);
        self.max_retries
            .store(settings.max_retries, Ordering::Release);
        self.generate_sha256
            .store(settings.generate_sha256, Ordering::Release);
        self.pause_during_postprocess
            .store(settings.pause_during_postprocess, Ordering::Release);
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
        // The hand-set limit is independent of the schedule: it stays in force through
        // every profile switch, and whichever of the two is stricter wins.
        self.config
            .bandwidth
            .limits()
            .set_manual_limit(settings.speed_limit_bytes_per_second);
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
        let Ok(files) = self.database.list_downloads().await else {
            return;
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

    /// Requeues what disabling those kinds had blocked — and only that.
    ///
    /// The kind alone is not enough of a filter: a file of a re-enabled kind may also be
    /// blocked because its storage root is full or because its validators changed mid-transfer,
    /// and switching the kind back on is not a verdict on either of those.
    async fn requeue_blocked_of_kinds(&self, kinds: &[rd_core::DownloadKind]) {
        let Ok(blocked) = self
            .database
            .downloads_blocked_by(BlockReason::KindDisabled.as_str())
            .await
        else {
            return;
        };
        let Ok(files) = self.database.list_downloads().await else {
            return;
        };
        for file in files
            .into_iter()
            .filter(|file| kinds.contains(&file.kind) && blocked.contains(&file.id))
        {
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
        worker::build_client(self, account_id, proxy_profile_id, auth_profile, scope).await
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
                initial_state: DownloadState::Queued,
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
    async fn supervise_rates(&self) -> Result<()> {
        let files = self.database.list_downloads().await?;
        let observations = files
            .iter()
            .map(|file| rates::RateObservation {
                id: file.id,
                committed_bytes: file.committed_bytes.get(),
                // Only a running transfer moves bytes. Verifying, repairing, extracting and
                // seeding do not, and neither does anything that is waiting.
                transferring: file.state == DownloadState::Downloading,
            })
            .collect::<Vec<_>>();
        self.rates.observe(std::time::Instant::now(), &observations);
        Ok(())
    }

    /// The current smoothed rate of every download in the queue, in bytes per second.
    ///
    /// Empty until the supervise loop has sampled twice; a rate needs two readings to exist.
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
        let destinations = self.package_destinations().await?;
        let files = self.database.list_downloads().await?;
        // Cloned once for the mirror check below, which needs to look at a file's siblings
        // while the loop has already taken the list apart.
        let all_files = files.clone();
        for file in files.into_iter().filter(|file| {
            file.state == DownloadState::Queued
                || (file.state == DownloadState::RetryWait
                    && file.next_retry_at.is_some_and(|retry_at| retry_at <= now))
        }) {
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
                let siblings = mirrors::siblings(&file, &all_files);
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
                self.block_queued_of_kind(file.kind).await;
                continue;
            }
            let external = match file.kind {
                rd_core::DownloadKind::Http => None,
                kind => {
                    let Some(runner) = self.runners.get(kind) else {
                        continue;
                    };
                    let Some(permit) = self.runners.try_slot(kind).await else {
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
            let cancellation = CancellationToken::new();
            {
                let mut active = self.active.lock().await;
                if active.tokens.contains_key(&file.id) || active.reasons.contains_key(&file.id) {
                    continue;
                }
                // Exempt kinds (recordings) start regardless of the global cap, so keep
                // scanning instead of breaking when the cap is reached.
                if !exempt && active.tokens.len() - active.exempt.len() >= active_limit {
                    continue;
                }
                active.tokens.insert(file.id, cancellation.clone());
                if exempt {
                    active.exempt.insert(file.id);
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

    async fn run_file(
        &self,
        file: rd_core::DownloadFile,
        cancellation: CancellationToken,
        runner: Option<Arc<dyn ExternalRunner>>,
    ) {
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
        let result = tracing::Instrument::instrument(
            async {
                match runner {
                    Some(runner) => self.run_external(runner, &file, cancellation).await,
                    None => worker::run(self, &file, cancellation).await,
                }
            },
            span,
        )
        .await;
        if let Err(error) = &result {
            tracing::warn!(download_id = %file.id, %error, "download attempt failed");
            if let Ok(Some(current)) = self.database.get_download(file.id).await
                && matches!(
                    current.state,
                    DownloadState::Resolving
                        | DownloadState::Downloading
                        | DownloadState::Verifying
                        | DownloadState::Repairing
                        | DownloadState::Extracting
                )
            {
                let failure = Failure::new(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    error.to_string(),
                );
                let retry_at = retry::retry_at(&failure, current.retry_count, self.max_retries());
                if let Err(error) = self
                    .database
                    .record_failure(file.id, failure, retry_at)
                    .await
                {
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
}
