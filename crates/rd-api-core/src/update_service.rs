//! The application update check as a service (RD-180-01): the periodic check, "check now", and
//! the status the interface, the REST route and the MCP tool read.
//!
//! The check itself is `rd_update`; this holds what it needs between runs — the per-channel
//! replay floors and the last result — in the settings table under [`STATE_KEY`], so a restart
//! neither forgets a floor nor checks again at once. Floors and result are one document written
//! in one statement, before anything acts on what verified, so a crash can never leave a newer
//! manifest accepted with the older floor stored.
//!
//! **The start is never held up.** [`UpdateService::start`] spawns the loop and returns; its
//! first check waits [`STARTUP_DELAY`] plus up to [`STARTUP_JITTER_SECONDS`], so installations
//! restarted together do not ask GitHub in the same second, and it re-reads the settings every
//! [`TICK`] so a changed interval or channel applies without a restart.
//!
//! **A build without the release key** checks nothing and says `configured: false`; no request
//! leaves for an answer that would be refused anyway.
//!
//! **The self-update** (RD-180-02) keeps its state here too: until the updater takes over, what
//! the service is doing (downloading, the backup before the update) in memory; from the hand-over
//! on, the journal in `<data>/update/`, which the updater, the restarted version and a start after
//! a crash all read. `rd_api_admin::update_install_service` runs the steps.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use rd_db::Database;
use rd_update::{
    Channel, Fetcher, Floors, HttpFetcher, InstallKind, Offer, Sources, Target, TrustStore,
    UpdateAction, UpdateError, UpdateSettings,
    install::{self, Journal},
};
use serde::{Deserialize, Serialize};

use crate::dto::{UpdateOffer, UpdateStatusResponse};

mod install_state;

/// The settings key the floors and the last result are stored under.
pub const STATE_KEY: &str = "update.state";
/// How long after the start the first check waits.
pub const STARTUP_DELAY: Duration = Duration::from_secs(180);
/// The most the first check is delayed beyond [`STARTUP_DELAY`], at random.
pub const STARTUP_JITTER_SECONDS: u64 = 180;
/// How often the loop looks whether a check is due.
pub const TICK: Duration = Duration::from_secs(600);

/// Starts the updater for a written journal (RD-180-02).
pub type Launcher = Arc<dyn Fn(&Journal) -> anyhow::Result<()> + Send + Sync>;

/// What the service does for an update before the updater takes over.
#[derive(Clone, Debug)]
struct Progress {
    /// `downloading`, `preparing`, `handed` (the journal speaks from here on) or `failed`.
    state: &'static str,
    target_version: String,
    reason: Option<String>,
    started_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// How this installation was installed, where its program lives and how its updater starts.
struct Installation {
    kind: InstallKind,
    /// The running executable's folder; `None` when it cannot be told.
    directory: Option<PathBuf>,
    launcher: Launcher,
}

/// What is kept between checks.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StoredState {
    #[serde(default)]
    pub floors: Floors,
    #[serde(default)]
    pub last_checked: Option<DateTime<Utc>>,
    /// The stable code of the last check's problem.
    #[serde(default)]
    pub last_error: Option<String>,
    /// The newest verified release above the version that ran the check.
    #[serde(default)]
    pub offer: Option<Offer>,
}

/// Where the manifests come from and what they are verified against.
struct Source {
    fetcher: Arc<dyn Fetcher>,
    sources: Sources,
    /// `None`: the compiled-in release root.
    trust: Option<TrustStore>,
}

/// The update check.
#[derive(Clone)]
pub struct UpdateService(Arc<Inner>);

struct Inner {
    database: Database,
    source: RwLock<Source>,
    installation: RwLock<Installation>,
    current: String,
    checking: tokio::sync::Mutex<()>,
    started: AtomicBool,
    progress: Mutex<Option<Progress>>,
}

impl UpdateService {
    /// A service over the official releases for the running build. Starts nothing.
    #[must_use]
    pub fn new(database: Database) -> Self {
        Self(Arc::new(Inner {
            database,
            source: RwLock::new(Source {
                fetcher: Arc::new(HttpFetcher::new()),
                sources: Sources::official(),
                trust: None,
            }),
            installation: RwLock::new(Installation {
                kind: InstallKind::detect(),
                directory: std::env::current_exe()
                    .ok()
                    .and_then(|executable| executable.parent().map(std::path::Path::to_path_buf)),
                launcher: Arc::new(install::process::launch_updater),
            }),
            current: env!("CARGO_PKG_VERSION").to_owned(),
            checking: tokio::sync::Mutex::new(()),
            started: AtomicBool::new(false),
            progress: Mutex::new(None),
        }))
    }

    /// Serves the manifests from `fetcher` at `sources`, verified against `trust`.
    ///
    /// For tests only, which serve signed manifests from memory under a key of their own; the
    /// service never calls it.
    #[doc(hidden)]
    pub fn use_source(&self, fetcher: Arc<dyn Fetcher>, sources: Sources, trust: TrustStore) {
        if let Ok(mut source) = self.0.source.write() {
            *source = Source {
                fetcher,
                sources,
                trust: Some(trust),
            };
        }
    }

    /// Whether anything can be verified: a test's trust store, or the compiled-in release root.
    #[must_use]
    pub fn configured(&self) -> bool {
        let overridden = self
            .0
            .source
            .read()
            .map(|source| source.trust.is_some())
            .unwrap_or(false);
        overridden || !release_root_missing()
    }

    /// The update slice of the settings, the defaults when it is malformed (and reported).
    pub async fn settings(&self) -> UpdateSettings {
        self.0
            .database
            .service_settings_or_default()
            .await
            .unwrap_or_default()
    }

    /// The channel actually read: a package manager that publishes no pre-releases is read on
    /// stable whatever was chosen, so it is never shown a command that installs nothing new.
    #[must_use]
    pub fn effective_channel(&self, settings: &UpdateSettings) -> Channel {
        match settings.channel() {
            Channel::Beta if self.install_kind().receives_betas() => Channel::Beta,
            _ => Channel::Stable,
        }
    }

    /// What was stored after the last check.
    pub async fn stored(&self) -> StoredState {
        match self.0.database.get_setting(STATE_KEY).await {
            Ok(Some(value)) => serde_json::from_value(value).unwrap_or_else(|error| {
                tracing::warn!(%error, "the stored update state is unreadable; starting over");
                StoredState::default()
            }),
            Ok(None) => StoredState::default(),
            Err(error) => {
                tracing::warn!(%error, "could not read the stored update state");
                StoredState::default()
            }
        }
    }

    /// Checks now, stores the result and returns it.
    ///
    /// A refused or unreachable manifest is part of the result (`last_error`), not an error of
    /// this call: the check ran and that is what it found. Only a build that cannot check at
    /// all ([`UpdateError::NotConfigured`]) and a failure to store the result are errors.
    pub async fn check(&self) -> Result<StoredState, UpdateError> {
        let _running = self.0.checking.lock().await;
        let (fetcher, sources, trust) = {
            let source =
                self.0.source.read().map_err(|_| {
                    UpdateError::Other(anyhow::anyhow!("update source lock poisoned"))
                })?;
            (
                Arc::clone(&source.fetcher),
                source.sources.clone(),
                source.trust.clone(),
            )
        };
        let now = Utc::now();
        let trust = match trust {
            Some(trust) => trust,
            None => rd_update::manifest::release_trust(now)?,
        };
        let channel = self.effective_channel(&self.settings().await);
        let previous = self.stored().await;
        let report = rd_update::check(
            fetcher.as_ref(),
            &sources,
            &trust,
            channel,
            previous.floors,
            now,
        )
        .await;
        let offer = if report.manifests.is_empty() {
            // Nothing verified this time: what was known stays known, the error says why it
            // could not be confirmed.
            previous.offer
        } else {
            rd_update::newest_offer(
                &self.0.current,
                channel,
                &report.manifests,
                &Target::current(self.install_kind()),
            )
        };
        let state = StoredState {
            floors: report.floors,
            last_checked: Some(now),
            last_error: report
                .problem
                .as_ref()
                .map(|problem| problem.code().to_owned()),
            offer,
        };
        self.0
            .database
            .set_setting(
                STATE_KEY.to_owned(),
                serde_json::to_value(&state).map_err(|error| UpdateError::Other(error.into()))?,
            )
            .await
            .map_err(UpdateError::Other)?;
        match &state.offer {
            Some(offer) => {
                tracing::info!(version = %offer.version, "a newer rDownloader is available")
            }
            None if state.last_error.is_none() => tracing::debug!("rDownloader is up to date"),
            None => {}
        }
        Ok(state)
    }

    /// The status as the interface shows it.
    pub async fn status(&self) -> UpdateStatusResponse {
        let settings = self.settings().await;
        let stored = self.stored().await;
        let configured = self.configured();
        let channel = self.effective_channel(&settings);
        let interval = settings.interval_hours();
        let next_check_at = (configured && settings.update_check_enabled)
            .then(|| {
                stored
                    .last_checked
                    .map(|last| last + chrono::Duration::hours(i64::from(interval)))
            })
            .flatten()
            .map(|at| at.to_rfc3339());
        // The stored offer was newer than the version that checked; after an update it may not
        // be newer than this one, and after a switch back to stable a beta is not offered.
        let available = stored
            .offer
            .filter(|offer| rd_update::is_newer(&offer.version, &self.0.current))
            .filter(|offer| channel == Channel::Beta || offer.channel == Channel::Stable)
            .map(|offer| self.describe(offer));
        UpdateStatusResponse {
            current_version: self.0.current.clone(),
            configured,
            check_enabled: settings.update_check_enabled,
            channel: settings.channel().as_str().to_owned(),
            effective_channel: channel.as_str().to_owned(),
            interval_hours: interval,
            install_kind: self.install_kind().as_str().to_owned(),
            checking: self.0.checking.try_lock().is_err(),
            last_checked_at: stored.last_checked.map(|at| at.to_rfc3339()),
            next_check_at,
            error_code: stored.last_error,
            available,
            install: self.install_status(),
        }
    }

    fn describe(&self, offer: Offer) -> UpdateOffer {
        let kind = self.install_kind();
        let (action, command, hint) = match kind.action(&offer.version) {
            // Installing needs this installation's artifact; without one it is a download.
            UpdateAction::Install if offer.artifact.is_some() => ("install", None, None),
            UpdateAction::Install | UpdateAction::Download => ("download", None, None),
            UpdateAction::Command { command, hint } => {
                ("command", Some(command), hint.map(str::to_owned))
            }
        };
        let rollback_available = (action == "install").then(|| {
            kind != InstallKind::Msi
                || install::steps::kept_installer(&self.data_dir(), &self.0.current).is_some()
        });
        UpdateOffer {
            release_url: format!(
                "https://github.com/{}/releases/tag/v{}",
                rd_update::check::OFFICIAL_REPOSITORY,
                offer.version
            ),
            channel: offer.channel.as_str().to_owned(),
            released_at: offer.released_at.to_rfc3339(),
            download_url: offer.artifact.as_ref().map(|artifact| artifact.url.clone()),
            download_size: offer.artifact.as_ref().map(|artifact| artifact.size),
            download_sha256: offer
                .artifact
                .as_ref()
                .map(|artifact| artifact.sha256.clone()),
            version: offer.version,
            notes: offer.notes,
            action: action.to_owned(),
            command,
            hint,
            rollback_available,
        }
    }

    /// Starts the periodic check, once. Returns at once; see the module documentation.
    pub fn start(&self) {
        if self.0.started.swap(true, Ordering::SeqCst) {
            return;
        }
        if !self.configured() {
            tracing::info!("this build carries no update signing key; the update check is off");
            return;
        }
        let service = self.clone();
        tokio::spawn(async move {
            let jitter = {
                use rand::RngExt;
                rand::rng().random_range(0..=STARTUP_JITTER_SECONDS)
            };
            tokio::time::sleep(STARTUP_DELAY + Duration::from_secs(jitter)).await;
            loop {
                if service.due().await
                    && let Err(error) = service.check().await
                {
                    tracing::warn!(code = error.code(), %error, "the update check failed");
                }
                tokio::time::sleep(TICK).await;
            }
        });
    }

    /// Whether the automatic check should run now.
    async fn due(&self) -> bool {
        let settings = self.settings().await;
        if !settings.update_check_enabled {
            return false;
        }
        let interval = chrono::Duration::hours(i64::from(settings.interval_hours()));
        self.stored()
            .await
            .last_checked
            .is_none_or(|last| Utc::now() - last >= interval)
    }
}

/// Whether this build lacks the release root (a fork or a build with the key blanked).
fn release_root_missing() -> bool {
    rd_update::manifest::release_trust(Utc::now()).is_err()
}
