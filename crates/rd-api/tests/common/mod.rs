//! Shared harness for the rd-api integration tests.
//!
//! Extracted rather than copied a sixth time: every test file needs the same full service
//! graph, and a per-file copy meant a change to `AppState` had to be applied five times. Seven
//! files still carried their own copy until RD-140-21; what made each one different — the
//! login switched on, a parked scheduler, a token with a narrower scope, the state itself — is
//! an [`Options`] now, so the next difference is one more option rather than an eighth copy.
//!
//! The request helpers live in `requests`, the waiting in `wait`, fixtures in `fixtures`; all of
//! it is re-exported here, so a test file names `common::…` and nothing deeper.

#![allow(dead_code, unused_imports)]

mod fixtures;
mod requests;
mod wait;

pub use fixtures::*;
pub use requests::*;
pub use wait::*;

use sha2::{Digest, Sha256};

/// Bearer the capture routes accept in tests.
pub const CAPTURE_BEARER: &str = "test-capture-bearer-token";
/// Bearer holding the full `api:*` scope.
pub const API_BEARER: &str = "test-api-bearer-token";
/// Bearer holding the read-only `api:read` scope.
pub const READ_BEARER: &str = "test-read-bearer-token";

/// The router plus the stores behind it, for tests that reopen the database.
pub struct Harness {
    pub router: axum::Router,
    /// The state the router was built from, for a test that builds a second router over it.
    pub state: rd_api::AppState,
    /// Lets a test drive a batch check, and with it everything that keys off its completion.
    pub link_check: rd_api::LinkCheckService,
    pub database: rd_db::Database,
    pub secrets: rd_secrets::SecretStore,
    pub database_path: std::path::PathBuf,
    /// The daemon-side watchers, so a test can read what interval they follow (RD-110-31).
    pub hotfolders: rd_api::HotFolderService,
    /// The live scheduler, for a test that parks a resolver on its captcha broker.
    pub scheduler: rd_scheduler::SchedulerHandle,
}

/// What distinguishes one harness from another.
///
/// The default is the installation most tests want: login off, a live scheduler, the three
/// bearers above.
#[derive(Default)]
pub struct Options {
    login: bool,
    parked: bool,
    local_capture_fetches: bool,
    tokens: Vec<(&'static str, &'static str)>,
}

impl Options {
    /// The administrator login switched **on**.
    ///
    /// The default harness disables it so session routes are reachable without a password;
    /// scope and token tests need the opposite, because a disabled login waves every request
    /// through before a scope is ever consulted.
    pub fn login(mut self) -> Self {
        self.login = true;
        self
    }

    /// A scheduler that never dispatches a queued row.
    ///
    /// For tests that fake a download's lifecycle with `transition_download` or a direct
    /// `UPDATE`: the live supervisor claims every `queued` row within half a second, and a test
    /// racing it for the same row lost on slow Windows runners (2026-09-26). With
    /// `max_active_files` at zero the dispatch loop skips every job that counts against the
    /// cap; nothing in the harness raises it again.
    pub fn parked(mut self) -> Self {
        self.parked = true;
        self
    }

    /// `capture/file` may fetch from loopback, where the tests' "indexers" listen; the service
    /// refuses that otherwise (`AppState::with_local_capture_fetches`).
    pub fn local_capture_fetches(mut self) -> Self {
        self.local_capture_fetches = true;
        self
    }

    /// One more bearer, holding `scope` alone, besides the three every harness has.
    pub fn token(mut self, bearer: &'static str, scope: &'static str) -> Self {
        self.tokens.push((bearer, scope));
        self
    }
}

/// Builds the full router against a temporary database and secret store.
pub async fn test_router(directory: &std::path::Path) -> axum::Router {
    test_harness(directory).await.router
}

/// Same, but keeps the database and secret store reachable.
pub async fn test_harness(directory: &std::path::Path) -> Harness {
    harness(directory, Options::default()).await
}

/// A harness whose scheduler never dispatches a queued row; see [`Options::parked`].
pub async fn parked_harness(directory: &std::path::Path) -> Harness {
    harness(directory, Options::default().parked()).await
}

/// A harness with the administrator login switched on; see [`Options::login`].
pub async fn auth_harness(directory: &std::path::Path) -> Harness {
    harness(directory, Options::default().login()).await
}

/// The harness `options` describe.
pub async fn harness(directory: &std::path::Path, options: Options) -> Harness {
    let database_path = directory.join("api-test.sqlite3");
    let database = rd_db::Database::open(&database_path)
        .await
        .expect("database");
    // Reopening the same directory builds a second router over the same database, which is
    // how a restart is tested; the token from the first run is already there.
    let standard = [
        (CAPTURE_BEARER, rd_core::CAPTURE_SCOPE),
        (API_BEARER, rd_core::API_SCOPE),
        (READ_BEARER, rd_core::API_READ_SCOPE),
    ];
    for (bearer, scope) in standard.into_iter().chain(options.tokens) {
        let token = database
            .create_capture_token(
                rd_core::CaptureTokenId::new(),
                scope.to_owned(),
                hex::encode(Sha256::digest(bearer.as_bytes())),
                vec![scope.to_owned()],
            )
            .await;
        if let Err(error) = token {
            assert!(
                error.to_string().contains("UNIQUE constraint failed"),
                "token for {scope}: {error}"
            );
        }
    }
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let plugins = rd_plugin_host::PluginInstaller::new(
        directory.join("plugins"),
        rd_plugin_host::PluginVerifier::new(true),
    );
    let media_settings = rd_media::shared_settings(&database)
        .await
        .expect("media settings");
    let (_media_runner, media_probe) =
        rd_media::build(database.clone(), secrets.clone(), media_settings.clone());
    let gallery_settings = rd_gallery::shared_settings(&database)
        .await
        .expect("gallery settings");
    let stream_settings = rd_stream::shared_settings(&database)
        .await
        .expect("stream settings");
    let torrent_settings = rd_torrent::shared_settings(&database)
        .await
        .expect("torrent settings");
    let torrent = rd_torrent::TorrentService::start(
        database.clone(),
        torrent_settings.clone(),
        directory.to_path_buf(),
        directory.join("downloads"),
    );
    let mut scheduler_config =
        rd_scheduler::SchedulerConfig::for_directory(directory.join("downloads"));
    if options.parked {
        scheduler_config.max_active_files = 0;
    }
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        scheduler_config,
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let extraction = rd_extract::ExtractionService::start(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: directory.join("passwords.txt"),
            rar_timeout: std::time::Duration::from_secs(60),
            default_scripts_directory: directory.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    let state = rd_api::AppState::new(
        database.clone(),
        scheduler.clone(),
        secrets.clone(),
        plugins,
        extraction,
        media_settings,
        media_probe,
        gallery_settings,
        stream_settings,
        torrent,
        torrent_settings,
        rd_power::PowerService::default(),
        rd_core::PostprocessHold::new(),
        rd_api::RemoteServices::new(
            database.clone(),
            secrets.clone(),
            std::sync::Arc::new(tokio::sync::RwLock::new(rd_core::RemoteSettings::default())),
            rd_http::SharedNetworkDefaults::default(),
        ),
    );
    let state = if options.local_capture_fetches {
        state.with_local_capture_fetches()
    } else {
        state
    };
    // Capture intake authenticates with its own token; reading candidates back is a session
    // route, so the default stands in for an installation without an admin password.
    state.auth.set_disabled(!options.login);
    Harness {
        router: rd_api::router(state.clone()),
        link_check: state.link_check.clone(),
        hotfolders: state.hotfolders.clone(),
        state,
        database,
        secrets,
        database_path,
        scheduler,
    }
}
