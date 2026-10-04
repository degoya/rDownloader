//! Axum REST, SSE, authentication and embedded web assets.
//!
//! The assembly (RD-160-06): the router, the OpenAPI document, the event stream, the web assets and
//! the About page's licence list. The handlers live in the crates below — `rd-api-core`,
//! `rd-api-access`, `rd-api-intake`, `rd-api-queue`, `rd-api-admin`, `rd-api-compat` and
//! `rd-api-mcp` — so rustc compiles the areas side by side and a change rebuilds its area and
//! what assembles it, not the whole surface.

mod about;
mod event_stream;
mod handlers;
#[cfg(test)]
mod mcp_coverage;
mod openapi;
mod routes;
#[cfg(test)]
mod scope_policy_tests;
mod static_assets;

use std::net::SocketAddr;

use anyhow::Result;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer, trace::TraceLayer};

pub use rd_api_admin::{
    backup_service, diagnostics_checks, plugin_repository_handlers::prepare_plugin_repositories,
    plugin_update_policy::VersionChoicePolicy,
};
pub use rd_api_core::{
    ApiError, AppState, AuthService, BuildInfo, HotFolderService, LinkCheckService, RUNTIME_FIELDS,
    RefusedSetting, RemoteJobChoiceOutcome, RemoteJobDiscardOutcome, RemoteJobRefused,
    RemoteJobService, RemoteJobSubmitOutcome, RemoteServices, audit, diagnosed_settings,
    dto::SettingsResponse, local_control, oidc_client, password_reset, policy_rows, required_scope,
    runtime_settings, service_switches, startup_settings,
};
pub use rd_api_intake::{site_rules_service, site_rules_service::catalogue as site_rule_catalogue};

// The modules of the crates below, at this crate's root, so that a module here names them as
// `crate::…` exactly as it did while the HTTP surface was one crate (RD-160-06).
use rd_api_access::{
    api_tokens, audit_handlers, auth_flow_handlers, auth_profile_handlers,
    browser_session_handlers, login_handlers, mfa_handlers, oidc_handlers, oidc_settings_handlers,
    passkey_handlers, password_handlers, password_login_handlers, password_reset_handlers,
    session_handlers, setup_handlers,
};
use rd_api_admin::{
    about_page, automation_handlers, backup_destination_handlers, backup_handlers, config_handlers,
    data_reset_handlers, diagnostics_dto, diagnostics_handlers, lifecycle_handlers,
    notify_handlers, object_storage_handlers, plugin_bundled, plugin_handlers, plugin_lifecycle,
    plugin_repository_handlers, plugin_update_policy, providers_handlers, remote_handlers,
    restore_handlers, restore_uploads, routing_backup, settings_backup, settings_backup_crypto,
    settings_backup_dto, settings_handlers, stats_handlers, stats_retention_service,
    tools_handlers, update_handlers,
};
use rd_api_compat as compat;
use rd_api_core::{
    auth, automation_input, automation_service, client, container_upload, dto, error, hosters,
    postprocess_handlers, reconnect_service, storage_capacity, trace_context,
};
use rd_api_intake::{
    area_backup, candidate_handlers, captcha_handlers, capture_file, collector_handlers,
    container_handlers, indexer_handlers, indexer_search, nzb_handlers, regex_tester,
    remote_listing_handlers, site_rules_dto, site_rules_handlers, stream_handlers,
    stream_schedule_handlers, subscription_autoqueue, subscription_handlers,
};
use rd_api_mcp as mcp;
use rd_api_queue::{
    auto_remove_service, bandwidth_handlers, bandwidth_manual_handlers, capture_summary,
    collision_handlers, download_handlers, download_sources, duplicates, media_dto, media_handlers,
    metrics, nzb_remote_job_handlers, package_clear, package_handlers, power_handlers,
    queue_pause_handlers, reconnect_handlers, remote_job_handlers, replay_dto, replay_handlers,
    storage_handlers, torrent_control, torrent_handlers, torrent_trackers, usenet_handlers,
};

pub use rd_api_core::container_upload::PUBLIC_BODY_LIMIT_BYTES;

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
    // Runs the scheduled full backup, after marking a run the last stop interrupted (RD-160-01).
    backup_service::start(state.clone());
    metrics::mark_started();

    let public = Router::new()
        .route("/api/v1/health", get(handlers::health))
        .route("/api/v1/auth/status", get(login_handlers::auth_status))
        .route("/api/v1/auth/setup", post(login_handlers::setup))
        .route("/api/v1/auth/login", post(login_handlers::login))
        .route(
            "/api/v1/auth/passkey/challenge",
            post(passkey_handlers::passkey_challenge),
        )
        .route(
            "/api/v1/auth/passkey/login",
            post(passkey_handlers::passkey_login),
        )
        .route("/api/v1/auth/logout", post(session_handlers::logout))
        // Signing in through the identity provider (RD-190-15): two browser navigations, the
        // start and the provider's redirect back. Public like the password sign-in; the
        // callback's credential is its `state` together with the `rd_oidc` binding cookie of the
        // browser that started it (ADR 0021).
        .route("/api/v1/auth/oidc/start", get(oidc_handlers::oidc_start))
        .route(
            "/api/v1/auth/oidc/callback",
            get(oidc_handlers::oidc_callback),
        )
        // The provider's redirect back from an OAuth sign-in. Public because it arrives from the
        // provider's site, which a `SameSite=Strict` session cookie does not travel from; the
        // `state` it echoes is its credential (`rd_api_core::auth_flow_guard`, security audit
        // 2026-09-30, finding 5).
        .route(
            "/api/v1/oauth/callback",
            get(auth_flow_handlers::oauth_callback),
        )
        .route("/api/v1/openapi.json", get(handlers::openapi))
        // Nobody signed in reaches these, so nobody may send them the 65 MiB the upload routes
        // need: a sign-in is a few hundred bytes and a passkey assertion a few kilobytes
        // (security audit 2026-09-30, finding 8). Set closer to the handlers than the
        // service-wide limit below, so it is the one the body extractors read.
        .layer(DefaultBodyLimit::max(PUBLIC_BODY_LIMIT_BYTES));

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
        .route("/api/v1/capture/nzb", post(nzb_handlers::capture_nzb))
        .route("/api/v1/capture/ping", get(handlers::capture_ping))
        .route("/api/v1/capture/events", get(event_stream::capture_events))
        .route(
            "/api/v1/capture/summary",
            get(capture_summary::capture_summary),
        )
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
    //
    // The host check goes in front of everything, the mount point included: a request under a
    // name this service does not answer to is refused whatever path it asks for, the web
    // interface's own files among them (security review 2026-09-28, finding 3).
    Router::new().fallback_service(
        tower::ServiceBuilder::new()
            .layer(middleware::from_fn_with_state(
                state.clone(),
                rd_api_core::host_check::require_known_host,
            ))
            .layer(middleware::from_fn_with_state(
                state,
                client::strip_base_path,
            ))
            .service(application),
    )
}

/// How long the open connections have to finish once the service stops.
///
/// The graceful stop waits for every response that is still being sent. The event streams end
/// with the stop themselves, and the MCP sessions with them; this bounds everything else --
/// a large file being served, a client that stopped reading -- so the stop never waits on a
/// peer. A cut-off response is what a client sees of a service that went away anyway.
pub const CONNECTION_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Runs the service on `address` until `state.shutdown` is cancelled.
pub async fn serve(state: AppState, address: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    serve_on(state, listener).await
}

/// Runs the service on a bound `listener` until `state.shutdown` is cancelled -- by a signal,
/// by `POST /api/v1/system/shutdown`, or by the updater through that route (RD-180-02).
///
/// Returns once every connection ended, or [`CONNECTION_DRAIN`] after the stop at the latest.
pub async fn serve_on(state: AppState, listener: tokio::net::TcpListener) -> Result<()> {
    state.auth.load(&state).await?;
    rd_api_core::host_check::load(&state).await;
    if state.auth.disabled() {
        tracing::warn!(
            "administrator login is disabled in the settings; every client on this machine is \
             trusted, every other one has to sign in"
        );
    }
    let bound = listener.local_addr()?;
    tracing::info!(address = %bound, "rDownloader listening");
    let shutdown = state.shutdown.clone();
    // With connect info, so a handler can see who is actually calling. Without it the peer
    // address is unreachable anywhere in the application, which makes both rate limiting and
    // the session inventory impossible to do honestly — the first would have nothing to key
    // on and the second nothing to show. The bound address beside it tells a peer on this
    // machine's own interface address from another machine (`client::from_this_machine`).
    let server = axum::serve(
        listener,
        router(state)
            .layer(axum::Extension(rd_api_core::client::ListenAddress(bound)))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.clone().cancelled_owned());
    let drained = async {
        shutdown.cancelled().await;
        tokio::time::sleep(CONNECTION_DRAIN).await;
    };
    tokio::select! {
        result = std::future::IntoFuture::into_future(server) => result?,
        () = drained => tracing::warn!(
            seconds = CONNECTION_DRAIN.as_secs(),
            "connections were still open after the stop; the service ends without them"
        ),
    }
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
