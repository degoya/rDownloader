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
mod dispatch;
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
mod network;
mod profile_boundary;
mod provider;
mod queue_pause;
mod rates;
mod replay;
mod retry;
#[cfg(test)]
mod run_guard_tests;
mod runner;
mod settings;
mod start;
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
};

use rd_core::{PluginId, ProxyProfileId};
use rd_db::Database;
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
pub use queue_pause::{QueuePause, pausable};
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
    /// Default download directory used when no category applies.
    #[must_use]
    pub fn downloads_directory(&self) -> &std::path::Path {
        &self.config.downloads_directory
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
}
