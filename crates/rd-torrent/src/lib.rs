//! BitTorrent engine embedded via librqbit: one shared session (DHT, trackers, PEX,
//! seeding), an [`ExternalRunner`] driving one queue row per torrent, and the seeding
//! service that completes rows once the ratio or time limit is reached.
//!
//! Known gap: librqbit has no web-seed (BEP 19) support.

mod bencode;
mod error;
mod forget;
mod metadata;
mod network;
mod plan;
mod prefetch;
mod priority;
mod proxy;
mod registry;
mod runner;
mod seeding;
mod session;
mod stats;
mod trackers;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use rd_core::TorrentSettings;
use rd_db::Database;
use rd_scheduler::ExternalRunner;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub use error::{TorrentError, TorrentErrorKind, torrent_kind};
pub use forget::TorrentLocation;
pub use metadata::{ParsedTorrent, parse_torrent};
pub use network::{INTERFACE_CHECK_INTERVAL, NetworkInterface, TorrentNetworkStatus, interfaces};
pub use prefetch::PREFETCH_TTL;
pub use runner::TorrentRunner;
pub use session::CAPABILITIES;
pub use stats::{STATS_INTERVAL, peer_page_size};
pub use trackers::{REANNOUNCE_INTERVAL, SCRAPE_FRESHNESS, is_stale};

/// Maximum accepted size of one uploaded `.torrent` metadata file.
pub const MAX_TORRENT_BYTES: usize = 16 * 1024 * 1024;

/// Settings shared between the engine and the settings endpoint. Port and rate limits
/// apply when the session is (re)created on the next start.
pub type SharedTorrentSettings = Arc<RwLock<TorrentSettings>>;

/// Reads the torrent settings from the `service.settings` blob.
///
/// Refuses a malformed blob rather than running on defaults: this is read once at start-up,
/// and defaulting here would silently open the session on another port and drop the rate
/// limits the person set.
pub async fn load_torrent_settings(database: &Database) -> Result<TorrentSettings> {
    database.service_settings().await
}

/// Creates the shared settings handle from the database.
pub async fn shared_settings(database: &Database) -> Result<SharedTorrentSettings> {
    Ok(Arc::new(RwLock::new(
        load_torrent_settings(database).await?,
    )))
}

pub(crate) struct ServiceInner {
    pub database: Database,
    pub settings: SharedTorrentSettings,
    /// Session persistence and stored `.torrent` files live below this directory.
    pub data_dir: PathBuf,
    /// Session default; every torrent overrides it with its package folder.
    pub default_output: PathBuf,
    /// The live session, rebuildable when a construction-time setting changes.
    session: RwLock<Option<session::SessionSlot>>,
    /// Incarnation counter handed to every session build.
    generation: std::sync::atomic::AtomicU64,
    /// Which queue row maps to which torrent, for downloading and seeding rows alike.
    pub registry: RwLock<registry::Registry>,
    /// Wakes the seeding supervisor when a policy changes, so a lowered limit takes effect
    /// immediately instead of at the next tick.
    pub seeding_nudge: tokio::sync::Notify,
    /// Whether the kill switch currently holds torrent traffic.
    pub kill_switch_engaged: std::sync::atomic::AtomicBool,
    /// Why the last session rebuild failed; the previous session keeps running.
    pub rebuild_error: RwLock<Option<String>>,
    /// Vault handle used to resolve the proxy password; absent in tests.
    pub secrets: Option<rd_secrets::SecretStore>,
    /// Active bandwidth profile, so a scheduled limit reaches the engine too; absent in
    /// tests and when no profile is configured.
    pub bandwidth: Option<rd_scheduler::BandwidthService>,
    pub shutdown: CancellationToken,
}

/// Cloneable engine handle shared by the runner, the API and the seeding loop.
#[derive(Clone)]
pub struct TorrentService {
    pub(crate) inner: Arc<ServiceInner>,
}

impl TorrentService {
    /// Creates the service and starts the seeding supervision loop. The librqbit session
    /// itself is created lazily on first use.
    #[must_use]
    pub fn start(
        database: Database,
        settings: SharedTorrentSettings,
        data_dir: PathBuf,
        default_output: PathBuf,
    ) -> Self {
        let service = Self {
            inner: Arc::new(ServiceInner {
                database,
                settings,
                data_dir,
                default_output,
                session: RwLock::new(None),
                generation: std::sync::atomic::AtomicU64::new(0),
                registry: RwLock::new(registry::Registry::default()),
                seeding_nudge: tokio::sync::Notify::new(),
                kill_switch_engaged: std::sync::atomic::AtomicBool::new(false),
                rebuild_error: RwLock::new(None),
                secrets: None,
                bandwidth: None,
                shutdown: CancellationToken::new(),
            }),
        };
        tokio::spawn(seeding::supervise(service.clone()));
        tokio::spawn(stats::broadcast_loop(service.clone()));
        tokio::spawn(network::watch(service.clone()));
        tokio::spawn(session::watch_bandwidth(service.clone()));
        service
    }

    /// Attaches the secret store, so a configured SOCKS5 proxy can be resolved.
    ///
    /// Separate from `start` because the vault is not needed to run torrents, and the
    /// integration tests construct the service without one.
    #[must_use]
    pub fn with_secrets(mut self, secrets: rd_secrets::SecretStore) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.secrets = Some(secrets);
        }
        self
    }

    /// Attaches the bandwidth service, so the active profile's rates reach the engine.
    ///
    /// librqbit takes one session-wide rate, so a per-host or per-category torrent limit
    /// cannot be expressed; the bandwidth capability matrix says so rather than accepting
    /// such a limit and ignoring it.
    #[must_use]
    pub fn with_bandwidth(mut self, bandwidth: rd_scheduler::BandwidthService) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.bandwidth = Some(bandwidth);
        }
        self
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Directory where uploaded `.torrent` files are stored.
    #[must_use]
    pub fn torrent_file_directory(&self) -> PathBuf {
        self.inner.data_dir.join("torrents")
    }

    /// Parses and stores an uploaded `.torrent` below the service data directory.
    ///
    /// The directory is canonicalized before the final path is built. This is important when
    /// the service was started with its default relative `data/` path: queue sources use
    /// `file://` URLs, which can only be constructed from absolute paths.
    pub async fn store_torrent_file(&self, bytes: &[u8]) -> Result<(ParsedTorrent, PathBuf)> {
        anyhow::ensure!(
            bytes.len() <= MAX_TORRENT_BYTES,
            "torrent file exceeds the 16 MiB limit"
        );
        let parsed = parse_torrent(bytes)?;
        let directory = self.torrent_file_directory();
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| format!("create torrent directory {}", directory.display()))?;
        let directory = dunce::canonicalize(&directory)
            .with_context(|| format!("resolve torrent directory {}", directory.display()))?;
        let stored = directory.join(format!("{}.torrent", parsed.info_hash));
        tokio::fs::write(&stored, bytes)
            .await
            .with_context(|| format!("store torrent file {}", stored.display()))?;
        Ok((parsed, stored))
    }

    /// Deletes the stored `.torrent` file behind a finished download when the user opted
    /// out of keeping import history. Only touches `file://` sources inside the service's
    /// torrent directory; magnets and external paths are left alone.
    pub(crate) async fn discard_stored_torrent_file(&self, source: &url::Url) {
        if self.inner.settings.read().await.keep_import_history {
            return;
        }
        let Ok(path) = source.to_file_path() else {
            return;
        };
        let Ok(directory) = dunce::canonicalize(self.torrent_file_directory()) else {
            return;
        };
        if !path.starts_with(&directory) {
            return;
        }
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), %error, "removing stored torrent file failed");
        }
    }

    /// The persisted torrent state of one queue row, or the defaults when it has none.
    pub(crate) async fn job_state(&self, id: rd_core::DownloadId) -> rd_core::TorrentJobState {
        match self.inner.database.download_torrent_state(id).await {
            Ok(Some(state)) => state,
            Ok(None) => rd_core::TorrentJobState::default(),
            Err(error) => {
                tracing::warn!(download_id = %id, %error, "reading torrent state failed");
                rd_core::TorrentJobState::default()
            }
        }
    }

    /// Persists the torrent state of one queue row.
    pub(crate) async fn store_job_state(
        &self,
        id: rd_core::DownloadId,
        state: rd_core::TorrentJobState,
    ) {
        if let Err(error) = self
            .inner
            .database
            .set_download_torrent_state(id, state)
            .await
        {
            tracing::warn!(download_id = %id, %error, "writing torrent state failed");
        }
    }

    /// The add options for one queue row: the package folder plus everything the persisted
    /// plan and tracker list contribute.
    pub(crate) async fn add_options(
        &self,
        id: rd_core::DownloadId,
        destination: &str,
    ) -> librqbit::AddTorrentOptions {
        librqbit::AddTorrentOptions {
            // Retries and restarts resume on top of the already-written payload.
            overwrite: true,
            output_folder: Some(destination.to_owned()),
            // Deselected files are never requested and never preallocated.
            only_files: self.only_files(id).await,
            // The engine has no runtime tracker API, so the persisted list is applied on
            // every add and re-add.
            trackers: self.tracker_urls(id).await,
            ..Default::default()
        }
    }

    /// Re-adds seeding torrents after a restart so they keep uploading.
    pub async fn recover(&self) -> Result<()> {
        let downloads = self.inner.database.list_downloads().await?;
        let packages = self.inner.database.list_packages().await?;
        for file in downloads.into_iter().filter(|file| {
            file.kind == rd_core::DownloadKind::Torrent
                && file.state == rd_core::DownloadState::Seeding
        }) {
            let Some(package) = packages
                .iter()
                .find(|package| package.id == file.package_id)
            else {
                continue;
            };
            if let Err(error) = seeding::resume(self, &file, package).await {
                tracing::warn!(file = %file.file_name, %error, "seeding torrent could not be resumed");
            }
        }
        Ok(())
    }

    /// Stops seeding one row and completes it; `Ok(false)` when it was not seeding.
    pub async fn stop_seeding(&self, id: rd_core::DownloadId) -> Result<bool> {
        seeding::stop(self, id, "manual stop").await
    }
}

/// Builds the runner registered with the scheduler.
#[must_use]
pub fn build(service: TorrentService) -> Arc<dyn ExternalRunner> {
    Arc::new(TorrentRunner::new(service))
}
