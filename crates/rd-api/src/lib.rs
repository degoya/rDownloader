//! Axum REST, SSE, authentication and embedded web assets.

mod about;
mod api_tokens;
mod area_backup;
mod audit;
mod audit_dto;
mod audit_handlers;
mod auth;
mod auth_flow_handlers;
mod auth_flow_service;
mod auth_profile_handlers;
mod auto_remove_service;
mod automation_actions;
mod automation_context;
mod automation_handlers;
mod automation_service;
mod bandwidth_handlers;
mod browser_session;
mod browser_session_handlers;
mod captcha_handlers;
mod capture_fetch;
mod capture_file;
mod capture_sanitize;
mod client;
mod collector_crawl_verdict;
mod collector_enqueue;
mod collector_exclusions;
mod collector_handlers;
mod compat;
mod config_handlers;
mod container_handlers;
mod container_upload;
mod data_reset_handlers;
mod destination;
pub mod diagnostics_checks;
mod diagnostics_dto;
mod diagnostics_handlers;
mod dlc_import;
mod download_handlers;
mod dto;
mod error;
mod error_codes;
mod event_stream;
mod handlers;
mod hosters;
mod hotfolder_service;
mod link_check_cache;
mod link_check_probe;
mod link_check_service;
mod mcp;
mod media_dto;
mod media_handlers;
mod metrics;
mod metrics_format;
mod mfa_handlers;
mod notify_handlers;
mod notify_service;
mod nzb_zip;
mod openapi;
mod package_clear;
mod package_handlers;
mod passkey_handlers;
mod password_handlers;
mod plugin_handlers;
mod postprocess_handlers;
mod power_handlers;
mod power_service;
mod providers_handlers;
mod reconnect_decision;
mod reconnect_handlers;
mod reconnect_ip;
mod reconnect_service;
mod regex_tester;
mod remote_handlers;
mod remote_job_handlers;
mod remote_job_service;
mod remote_listing_handlers;
mod replay_dto;
mod replay_handlers;
mod routes;
mod routing_backup;
mod scope_policy;
mod session_handlers;
mod settings_backup;
mod settings_backup_auth;
mod settings_backup_crypto;
mod settings_backup_dto;
mod settings_backup_secrets;
mod setup_handlers;
mod site_rules_dto;
mod site_rules_handlers;
pub mod site_rules_service;
mod static_assets;
mod stats_handlers;
mod stats_retention_service;
mod storage_capacity;
mod stream_handlers;
mod stream_monitor;
mod stream_schedule_handlers;
mod subscription_autoqueue;
mod subscription_handlers;
mod subscription_hosts;
mod subscription_service;
mod tools_handlers;
mod torrent_control;
mod torrent_handlers;
mod torrent_trackers;
mod trace_context;
mod usenet_handlers;

use std::net::SocketAddr;

use anyhow::Result;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};
use rd_db::Database;
use rd_scheduler::SchedulerHandle;
use tokio_util::sync::CancellationToken;
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer, trace::TraceLayer};

pub use about::BuildInfo;
pub use auth::AuthService;
pub use error::ApiError;
pub use handlers::service_switches;
pub use hotfolder_service::HotFolderService;
pub use link_check_service::LinkCheckService;
pub use remote_job_service::{
    ChoiceOutcome as RemoteJobChoiceOutcome, DiscardOutcome as RemoteJobDiscardOutcome,
    RemoteJobRefused, RemoteJobService, SubmitOutcome as RemoteJobSubmitOutcome,
};
pub use scope_policy::{policy_rows, required_scope};
pub use site_rules_service::catalogue as site_rule_catalogue;

/// Shared state cloned into request handlers.
#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub scheduler: SchedulerHandle,
    pub auth: AuthService,
    pub hotfolders: HotFolderService,
    pub secrets: rd_secrets::SecretStore,
    pub plugins: rd_plugin_host::PluginInstaller,
    pub extraction: rd_extract::ExtractionService,
    pub link_check: link_check_service::LinkCheckService,
    /// Media (yt-dlp) settings shared with the runner and the probe.
    pub media_settings: rd_media::SharedMediaSettings,
    /// Gallery (gallery-dl) settings shared with the runner.
    pub gallery_settings: rd_gallery::SharedGallerySettings,
    /// Recording (streamlink) settings shared with the runner and the channel monitor.
    pub stream_settings: rd_stream::SharedStreamSettings,
    /// Managed external tools (RD-102-02): the signed manifest, the installed versions and
    /// which one the tool lookup answers with.
    pub tools: rd_tools::ManagedToolService,
    /// Background monitor that starts recordings when watched channels go live.
    pub stream_monitor: stream_monitor::StreamMonitorService,
    pub subscriptions: subscription_service::SubscriptionService,
    /// Embedded BitTorrent engine (session, seeding supervision).
    pub torrent: rd_torrent::TorrentService,
    /// Torrent settings shared with the engine (port/limits apply on next start).
    pub torrent_settings: rd_torrent::SharedTorrentSettings,
    /// Quiet hours, completion actions and the platform power/network context.
    pub power: rd_power::PowerService,
    /// Raised while quiet hours defer the resource-intensive post-processing steps.
    pub quiet_hold: power_service::QuietHold,
    /// Background loop that watches the queue for a completed work cycle.
    pub power_supervisor: power_service::PowerSupervisor,
    /// Notification hub: event mapping and the delivery worker.
    pub notifications: notify_service::NotificationService,
    /// Automation engine: event intake, run queue and action execution.
    pub automations: automation_service::AutomationService,
    /// FTP/FTPS transport, used by the online check and the credential test action.
    pub ftp: rd_ftp::FtpService,
    /// SFTP transport, which also owns the SSH host-key trust decisions.
    pub sftp: rd_sftp::SftpService,
    /// Remote transfer settings shared with both runners.
    pub remote_settings: rd_ftp::SharedRemoteSettings,
    /// URL schemes the installed transfer backends claim, so intake can route a link to the
    /// plugin runner. Read once at startup: a newly installed backend needs a restart before
    /// its resolver or runner exists anyway.
    pub plugin_transfer_schemes: std::sync::Arc<Vec<String>>,
    /// Installed intake parsers (RD-090-12). Read once at startup, like the transfer
    /// schemes: a newly installed parser needs a restart before it can be built anyway.
    pub intake_parsers: std::sync::Arc<rd_plugin_ext::IntakeParsers>,
    /// Installed folder crawlers (RD-104-03), read once at startup like the parsers. They
    /// turn a pasted folder address into the files behind it before anything else looks at
    /// the link.
    pub crawlers: std::sync::Arc<rd_plugin_ext::FolderCrawlers>,
    /// The site rules a watched release page asks about its listing (RD-110-21). Filled by
    /// [`AppState::with_crawlers`], because the poll loop exists before the rules do.
    pub site_rule_claims: subscription_service::SharedSiteRules,
    /// Provider sign-in flows run by authentication plugins (RD-090-13).
    pub auth_flows: auth_flow_service::AuthFlowService,
    /// Jobs that run at a provider (RD-108-03): the sweep that drives them and the entry
    /// point that starts one.
    pub remote_jobs: remote_job_service::RemoteJobService,
    pub reconnect: reconnect_service::ReconnectService,
    /// Requests for a browser's session at a provider, opened in the web interface and
    /// answered by the extension (RD-120-45). In memory only.
    pub browser_sessions: browser_session::BrowserSessions,
    /// Installed post-processing step plugins (RD-090-16), for the settings and category
    /// editors. The pipeline reaches them through the runner it was started with.
    pub plugin_steps: std::sync::Arc<rd_plugin_ext::PluginSteps>,
    /// Installed upload destination plugins (RD-090-17), for the settings editor. The
    /// pipeline reaches them through the uploader it was started with.
    pub storage_destinations: std::sync::Arc<rd_plugin_ext::StorageDestinations>,
    /// The contract with whatever sits in front of the service: which hops may speak for a
    /// client, what the outside world calls this installation, and whether cookies are Secure.
    ///
    /// Empty by default, and that default is the safe one: with nothing configured the
    /// socket's peer address is the client and no header can change it. Behind a lock because
    /// the setting is editable while the service runs.
    pub proxy: std::sync::Arc<tokio::sync::RwLock<rd_authn::ProxyConfig>>,
    /// Passkey enrolments waiting for the browser's half of the ceremony.
    ///
    /// In memory rather than in the database: `webauthn-rs` refuses to serialise this state
    /// without an explicitly dangerous feature flag, because a challenge that survives a
    /// restart is a challenge that can be replayed into the process that comes back.
    pub(crate) passkey_registrations:
        std::sync::Arc<rd_authn::CeremonyStore<webauthn_rs::prelude::PasskeyRegistration>>,
    /// Passkey sign-ins waiting for the same. Reachable without a session, hence bounded.
    pub(crate) passkey_authentications:
        std::sync::Arc<rd_authn::CeremonyStore<webauthn_rs::prelude::PasskeyAuthentication>>,
    /// Live `SID` handles of the qBittorrent adapter.
    ///
    /// In memory, like the passkey ceremonies above and for a related reason: a handle that
    /// cannot outlive the process it was minted in cannot be replayed into the next one. The
    /// cost is that an automation client logs in again after a restart, which is what real
    /// qBittorrent makes it do anyway.
    pub(crate) qbittorrent_sessions: std::sync::Arc<compat::qbittorrent::sessions::SessionStore>,
    /// Commit and build time for the About page (RD-130-12); empty unless the binary sets them.
    pub build: std::sync::Arc<BuildInfo>,
}

/// The milestone 0.6 transfer services, grouped so they travel as one argument.
#[derive(Clone)]
pub struct RemoteServices {
    pub ftp: rd_ftp::FtpService,
    pub sftp: rd_sftp::SftpService,
    pub settings: rd_ftp::SharedRemoteSettings,
}

impl RemoteServices {
    /// Builds both transports on the scheduler's network defaults, so a proxy and a custom
    /// CA reach them exactly as they reach HTTP. Byte pacing is not passed here: the queue
    /// hands each runner a limiter scoped to the transfer it is about to start.
    #[must_use]
    pub fn new(
        database: Database,
        secrets: rd_secrets::SecretStore,
        settings: rd_ftp::SharedRemoteSettings,
        network_defaults: rd_http::SharedNetworkDefaults,
    ) -> Self {
        Self {
            ftp: rd_ftp::FtpService::new(
                database.clone(),
                secrets.clone(),
                settings.clone(),
                network_defaults,
            ),
            sftp: rd_sftp::SftpService::new(database, secrets, settings.clone()),
            settings,
        }
    }
}

impl AppState {
    /// Creates API state from initialized service components.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: Database,
        scheduler: SchedulerHandle,
        secrets: rd_secrets::SecretStore,
        plugins: rd_plugin_host::PluginInstaller,
        extraction: rd_extract::ExtractionService,
        media_settings: rd_media::SharedMediaSettings,
        media_probe: std::sync::Arc<dyn rd_media::MediaProbe>,
        gallery_settings: rd_gallery::SharedGallerySettings,
        stream_settings: rd_stream::SharedStreamSettings,
        torrent: rd_torrent::TorrentService,
        torrent_settings: rd_torrent::SharedTorrentSettings,
        power: rd_power::PowerService,
        quiet_hold: rd_core::PostprocessHold,
        remote: RemoteServices,
    ) -> Self {
        // Built before the hotfolders: a `.dlc` dropped into a folder is checked through the
        // same service a pasted batch goes through.
        let link_check = link_check_service::LinkCheckService::start(
            database.clone(),
            scheduler.clone(),
            media_probe.clone(),
            remote.ftp.clone(),
            remote.sftp.clone(),
            torrent.clone(),
            plugins.clone(),
            scheduler.plugin_host(),
        );
        let hotfolders = HotFolderService::new(
            database.clone(),
            scheduler.clone(),
            torrent.clone(),
            secrets.clone(),
            link_check.clone(),
            media_settings.clone(),
            gallery_settings.clone(),
        );
        let stream_monitor = stream_monitor::StreamMonitorService::start(
            database.clone(),
            scheduler.clone(),
            stream_settings.clone(),
        );
        // The media adapter reuses the existing probe rather than talking to yt-dlp a second
        // way; RD-080-10, RD-080-11, RD-110-21 and RD-130-19 add their adapters to this same list.
        let site_rule_claims = subscription_service::SharedSiteRules::default();
        let subscription_adapters: Vec<std::sync::Arc<dyn rd_subscription::SourceAdapter>> = vec![
            std::sync::Arc::new(rd_subscription::MediaAdapter::new(media_probe.clone())),
            std::sync::Arc::new(rd_subscription::FeedAdapter::new(std::sync::Arc::new(
                subscription_service::HttpFeedFetcher::new(scheduler.clone()),
            ))),
            std::sync::Arc::new(rd_subscription::IndexerAdapter::new(
                std::sync::Arc::new(subscription_service::HttpFeedFetcher::new(
                    scheduler.clone(),
                )),
                std::sync::Arc::new(subscription_service::VaultSecretResolver::new(
                    secrets.clone(),
                )),
            )),
            // A watched release or series page (RD-110-21): the listing is fetched
            // conditionally like a feed, and which of its links are release pages is the
            // rules' answer rather than a guess.
            std::sync::Arc::new(rd_subscription::RuleAdapter::new(
                std::sync::Arc::new(subscription_service::HttpFeedFetcher::new(
                    scheduler.clone(),
                )),
                std::sync::Arc::new(site_rule_claims.clone()),
            )),
            // An administrator's script whose output lines are links (RD-130-19), run through
            // the same sandbox as every other user script.
            std::sync::Arc::new(rd_subscription::ScriptAdapter::new(std::sync::Arc::new(
                subscription_service::SandboxScriptRunner::new(extraction.clone()),
            ))),
        ];
        let subscriptions = subscription_service::SubscriptionService::start(
            database.clone(),
            link_check.clone(),
            media_settings.clone(),
            gallery_settings.clone(),
            subscription_adapters,
        );
        let quiet_hold = power_service::QuietHold::new(quiet_hold);
        let auth_flows = auth_flow_service::AuthFlowService::start(
            database.clone(),
            plugins.clone(),
            scheduler.plugin_host(),
        );
        let remote_jobs = remote_job_service::RemoteJobService::start(
            database.clone(),
            plugins.clone(),
            scheduler.plugin_host(),
            link_check.clone(),
        );
        link_check.attach_cache_checkers(remote_jobs.clone());
        let notifications = notify_service::NotificationService::start(
            database.clone(),
            secrets.clone(),
            power.clone(),
            plugins.clone(),
            scheduler.plugin_host(),
        );
        let automations = automation_service::AutomationService::start(
            database.clone(),
            secrets.clone(),
            scheduler.clone(),
            extraction.clone(),
        );
        // Rooted at the data directory when the binary registered one. `data/` is the same
        // default the CLI uses, so a test that never sets one reads an empty store rather
        // than an absolute path from somewhere else.
        let tools = rd_tools::ManagedToolService::new(
            database.clone(),
            rd_tools::store_root(
                rd_core::data_directory().unwrap_or_else(|| std::path::Path::new("data")),
            ),
            rd_core::ManagedToolSettings::default(),
        );
        let power_supervisor = power_service::PowerSupervisor::start(
            database.clone(),
            scheduler.clone(),
            extraction.clone(),
            power.clone(),
            quiet_hold.clone(),
        );
        Self {
            database,
            scheduler,
            auth: AuthService::default(),
            proxy: std::sync::Arc::new(tokio::sync::RwLock::new(rd_authn::ProxyConfig::default())),
            passkey_registrations: std::sync::Arc::new(rd_authn::CeremonyStore::new()),
            passkey_authentications: std::sync::Arc::new(rd_authn::CeremonyStore::new()),
            qbittorrent_sessions: std::sync::Arc::default(),
            hotfolders,
            secrets,
            plugins,
            extraction,
            link_check,
            media_settings,
            gallery_settings,
            stream_settings,
            stream_monitor,
            tools,
            subscriptions,
            torrent,
            torrent_settings,
            power,
            quiet_hold,
            power_supervisor,
            notifications,
            auth_flows,
            remote_jobs,
            reconnect: reconnect_service::ReconnectService::default(),
            browser_sessions: browser_session::BrowserSessions::default(),
            automations,
            ftp: remote.ftp,
            sftp: remote.sftp,
            remote_settings: remote.settings,
            plugin_transfer_schemes: std::sync::Arc::new(Vec::new()),
            intake_parsers: std::sync::Arc::new(rd_plugin_ext::IntakeParsers::none()),
            crawlers: std::sync::Arc::new(rd_plugin_ext::FolderCrawlers::none()),
            site_rule_claims,
            plugin_steps: std::sync::Arc::new(rd_plugin_ext::PluginSteps::none()),
            storage_destinations: std::sync::Arc::new(rd_plugin_ext::StorageDestinations::none()),
            build: std::sync::Arc::default(),
        }
    }

    /// Reads the managed tool store from disk and makes it the managed stage of the tool
    /// lookup (RD-102-02).
    ///
    /// Separate from the constructor because it touches the filesystem: the pointers, the
    /// staging leftovers of an interrupted install and the cached manifest all have to be
    /// read before anything resolves a tool, and the resolver is registered only afterwards
    /// so no job can see a half-loaded store. A test that never calls this simply has no
    /// managed stage, which is exactly the behaviour of an installation that manages nothing.
    pub async fn prepare_managed_tools(&self, settings: rd_core::ManagedToolSettings) {
        self.tools.apply_settings(settings);
        self.tools.load().await;
        rd_core::set_managed_tool_resolver(std::sync::Arc::new(self.tools.clone()));
    }

    /// Records the installed post-processing step plugins.
    #[must_use]
    pub fn with_plugin_steps(mut self, steps: std::sync::Arc<rd_plugin_ext::PluginSteps>) -> Self {
        self.plugin_steps = steps;
        self
    }

    /// Records the installed upload destination plugins.
    #[must_use]
    pub fn with_storage_destinations(
        mut self,
        destinations: std::sync::Arc<rd_plugin_ext::StorageDestinations>,
    ) -> Self {
        self.storage_destinations = destinations;
        self
    }

    /// Records the installed intake parsers.
    ///
    /// Separate from the constructor for the same reason the transfer schemes are: they are
    /// discovered from the plugin directory rather than configured, and a service that
    /// cannot build one still has to start.
    #[must_use]
    pub fn with_intake_parsers(
        mut self,
        parsers: std::sync::Arc<rd_plugin_ext::IntakeParsers>,
    ) -> Self {
        self.intake_parsers = parsers;
        self
    }

    /// Records the installed folder crawlers (RD-104-03).
    ///
    /// Discovered rather than configured, exactly like the intake parsers: a service with
    /// none behaves as it did before there were any.
    #[must_use]
    pub fn with_crawlers(
        mut self,
        crawlers: std::sync::Arc<rd_plugin_ext::FolderCrawlers>,
    ) -> Self {
        // The same rules the crawler selection uses, so a watched listing and a pasted
        // address never disagree about which links are release pages (RD-110-21).
        self.site_rule_claims
            .set(crawlers.rules().map(std::sync::Arc::clone));
        self.crawlers = crawlers;
        self
    }

    /// Records the commit and build time the binary was compiled with (RD-130-12).
    ///
    /// Separate from the constructor because only the binary knows them: its build script
    /// compiles them in, and a test router simply has none.
    #[must_use]
    pub fn with_build_info(mut self, build: BuildInfo) -> Self {
        self.build = std::sync::Arc::new(build);
        self
    }

    /// Records the URL schemes the installed transfer backends claim.
    ///
    /// Separate from the constructor because it is discovered rather than configured: the
    /// backends are compiled at startup, and a service with none behaves exactly as before.
    #[must_use]
    pub fn with_plugin_transfer_schemes(mut self, schemes: Vec<String>) -> Self {
        self.plugin_transfer_schemes = std::sync::Arc::new(schemes);
        self
    }
}

/// OpenAPI document generated from the Rust handler contracts.
/// Builds the complete same-origin API and SPA router.
pub fn router(state: AppState) -> Router {
    // Completes the AutoQueue path: a subscription's links are promoted into the download
    // queue once their online check finishes. Started here so it exists for every way the
    // application is assembled, tests included.
    subscription_autoqueue::start(state.clone());
    // Removes finished packages on a delay, for the same reason and in the same place.
    auto_remove_service::start(state.clone());
    // Watches for free downloads stuck behind an address limit.
    state.reconnect.clone().start(state.clone());
    // Thins the persistent transfer statistics, and dates the uptime metric (RD-110-01).
    stats_retention_service::start(state.clone());
    metrics::mark_started();

    let public = Router::new()
        .route("/api/v1/health", get(handlers::health))
        .route("/api/v1/auth/status", get(handlers::auth_status))
        .route("/api/v1/auth/setup", post(handlers::setup))
        .route("/api/v1/auth/login", post(handlers::login))
        .route(
            "/api/v1/auth/passkey/challenge",
            post(passkey_handlers::passkey_challenge),
        )
        .route(
            "/api/v1/auth/passkey/login",
            post(passkey_handlers::passkey_login),
        )
        .route("/api/v1/auth/logout", post(session_handlers::logout))
        .route("/api/v1/openapi.json", get(handlers::openapi));

    let protected = routes::protected().route_layer(middleware::from_fn_with_state(
        state.clone(),
        auth::require_session,
    ));

    // Capture routes are token-authenticated and reachable from browser extensions,
    // userscripts and other tools: allow cross-origin calls (bearer header, JSON body).
    let capture_cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
        ]);
    let capture = Router::new()
        .route(
            "/api/v1/capture/batches",
            post(collector_handlers::capture_intake),
        )
        .route(
            "/api/v1/capture/cookies",
            post(auth_profile_handlers::capture_cookies),
        )
        .route(
            "/api/v1/capture/captchas",
            get(captcha_handlers::list_capture_captchas),
        )
        .route(
            "/api/v1/capture/captchas/{id}/token",
            post(captcha_handlers::answer_capture_captcha),
        )
        .route(
            "/api/v1/capture/captchas/{id}/skip",
            post(captcha_handlers::skip_capture_captcha),
        )
        .route(
            "/api/v1/capture/captchas/{id}/no-widget",
            post(captcha_handlers::report_capture_captcha_without_widget),
        )
        .route(
            "/api/v1/capture/browser-sessions",
            get(browser_session_handlers::list_capture_browser_sessions),
        )
        .route(
            "/api/v1/capture/browser-sessions/{id}",
            post(browser_session_handlers::deliver_capture_browser_session),
        )
        .route(
            "/api/v1/capture/browser-sessions/{id}/decline",
            post(browser_session_handlers::decline_capture_browser_session),
        )
        .route("/api/v1/capture/file", post(capture_file::capture_file))
        .route("/api/v1/capture/nzb", post(handlers::capture_nzb))
        .route("/api/v1/capture/ping", get(handlers::capture_ping))
        .route("/api/v1/capture/events", get(event_stream::capture_events))
        .route("/api/v1/capture/summary", get(handlers::capture_summary))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_capture,
        ))
        .layer(capture_cors);

    // MCP endpoint: one path serves POST (JSON-RPC), GET (SSE) and DELETE (session end).
    // No CORS on purpose — MCP clients are non-browser processes with a bearer token.
    let mcp_routes = Router::new()
        .route_service("/mcp", mcp::service(state.clone()))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_token,
        ));

    let application = public
        .merge(protected)
        .merge(capture)
        .merge(mcp_routes)
        .merge(compat::routes(&state))
        .fallback(static_assets::serve)
        // Leave room for multipart framing; the NZB handler enforces the exact 64 MiB file limit.
        // Axum's Multipart extractor otherwise caps bodies at its 2 MiB default, which broke
        // every larger NZB upload despite the tower-http layer below.
        .layer(DefaultBodyLimit::max(container_upload::BODY_LIMIT_BYTES))
        .layer(RequestBodyLimitLayer::new(
            container_upload::BODY_LIMIT_BYTES,
        ))
        // Outside the limit layer, so the bare 413 it answers a JSON request with gets a code.
        .layer(middleware::from_fn(container_upload::code_oversized_json))
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        // Establishes the trace every request belongs to (RD-110-03). Inside the HTTP
        // trace layer, so the span it opens is the parent of everything the handler does.
        .layer(middleware::from_fn(trace_context::attach))
        .with_state(state.clone());

    // The mount point has to be gone *before* anything routes on the path, and
    // `Router::layer` cannot do that: it wraps each route, so routing has already happened by
    // the time it runs. Wrapping the finished router as a service is what puts the middleware
    // in front of the routing instead of behind it.
    //
    // Doing it this way rather than nesting the routes under the base keeps every route
    // pattern — and therefore every `MatchedPath`, and therefore the whole scope policy —
    // written as `/api/v1/…` no matter where the service is mounted.
    Router::new().fallback_service(
        tower::ServiceBuilder::new()
            .layer(middleware::from_fn_with_state(
                state,
                client::strip_base_path,
            ))
            .service(application),
    )
}

/// Runs the service until the cancellation token fires.
pub async fn serve(
    state: AppState,
    address: SocketAddr,
    shutdown: CancellationToken,
) -> Result<()> {
    state.auth.load(&state).await?;
    if state.auth.disabled() {
        tracing::warn!("administrator login is disabled in the settings; every client is trusted");
    }
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "rDownloader listening");
    // With connect info, so a handler can see who is actually calling. Without it the peer
    // address is unreachable anywhere in the application, which makes both rate limiting and
    // the session inventory impossible to do honestly — the first would have nothing to key
    // on and the second nothing to show.
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await?;
    Ok(())
}

/// Returns the generated OpenAPI document.
#[must_use]
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    openapi::document()
}

/// Builds the request span with credential-bearing query parameters redacted.
///
/// The default span records the URI as it arrived, and the SABnzbd compatibility surface accepts
/// its API key in the query string — not by choice, but because that is the shape every real
/// SABnzbd client sends, so refusing it would break all of them. The key would therefore be
/// written into this service's own log, which is copied, shipped and read by people who have no
/// business holding an API token. Redacting it here covers the span before any subscriber sees
/// it; a reverse proxy in front still logs its own access line, which this cannot reach.
///
/// The names come from `rd_core::is_secret_parameter`, so the one list the project keeps of
/// credential-bearing parameters governs this too, rather than a second list that drifts.
fn request_span(request: &axum::http::Request<axum::body::Body>) -> tracing::Span {
    tracing::info_span!(
        "request",
        method = %request.method(),
        uri = %redact_uri(request.uri()),
        version = ?request.version(),
    )
}

/// The request target with every credential-bearing query value replaced.
///
/// A URI with nothing to hide is rendered byte-for-byte, so ordinary log lines keep their exact
/// text and only the ones carrying a secret change shape.
fn redact_uri(uri: &axum::http::Uri) -> String {
    let Some(query) = uri.query() else {
        return uri.to_string();
    };
    if !query
        .split('&')
        .any(|pair| rd_core::is_secret_parameter(pair.split('=').next().unwrap_or(pair)))
    {
        return uri.to_string();
    }
    let redacted = query
        .split('&')
        .map(|pair| {
            let name = pair.split('=').next().unwrap_or(pair);
            if rd_core::is_secret_parameter(name) {
                format!("{name}={}", rd_core::REDACTION_PLACEHOLDER)
            } else {
                pair.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{redacted}", uri.path())
}

#[cfg(test)]
mod redaction_tests {
    use super::redact_uri;

    #[test]
    fn a_traced_uri_keeps_everything_except_the_credential() {
        let plain = "/api?mode=queue&cat=tv".parse().expect("uri");
        assert_eq!(redact_uri(&plain), "/api?mode=queue&cat=tv");

        // The shape every SABnzbd client sends. Only the value goes; the mode still has to be
        // readable, or the log stops being useful for the thing logs are kept for.
        let keyed = "/sabnzbd/api?mode=addurl&apikey=7f3b&name=x"
            .parse()
            .expect("uri");
        assert_eq!(
            redact_uri(&keyed),
            "/sabnzbd/api?mode=addurl&apikey=[redacted]&name=x"
        );

        let bare = "/api".parse().expect("uri");
        assert_eq!(redact_uri(&bare), "/api");
    }
}
