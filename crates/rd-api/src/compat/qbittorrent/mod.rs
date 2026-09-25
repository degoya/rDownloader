//! A qBittorrent Web API v2 subset for automation clients.
//!
//! qBittorrent is the default download client of the *arr family, so this is the torrent
//! counterpart of the SABnzbd adapter. It answers the endpoints those clients call and
//! nothing else.
//!
//! Authentication follows qBittorrent's own shape — `POST /api/v2/auth/login` then a `SID`
//! cookie — but the credential is one of our revocable `api:*` tokens rather than a second
//! password. The cookie carries an opaque handle to that token, not the token: see
//! [`sessions`] for why the stateless version was given up. The handle still resolves to a
//! bearer that is checked against the token store on every request, so revoking the token
//! takes effect immediately; the cookie follows the same `Secure` and base-path policy as the
//! native session cookie, and the login is metered by the same limiter as the native one.

mod app;
mod map;
pub(crate) mod sessions;
mod torrents;

use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::AppState;

/// Cookie qBittorrent hands out after a successful login.
const SESSION_COOKIE: &str = "SID";

/// Login credentials. qBittorrent takes a username and a password; the username is ignored
/// here because a token identifies itself.
#[derive(Debug, Default, Deserialize)]
struct LoginForm {
    #[serde(default)]
    password: Option<String>,
}

/// Query parameters shared by the torrent endpoints.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct TorrentQuery {
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub hashes: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub filter: Option<String>,
}

/// The two routes that exist to *obtain* a credential, so they cannot require one.
fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v2/auth/login", post(login).get(login))
        .route("/api/v2/auth/logout", post(logout).get(logout))
}

/// Everything else, behind one layer.
///
/// Previously each of these handlers called [`guard`] itself — twenty-odd copies of the same
/// three lines, where forgetting one left the route open with nothing to say so. A layer
/// cannot be forgotten per route: a handler added to this router is authenticated because it
/// is in this router.
fn guarded_routes() -> Router<AppState> {
    Router::new()
        .route("/api/v2/app/version", get(app::version_handler))
        .route(
            "/api/v2/app/webapiVersion",
            get(app::web_api_version_handler),
        )
        .route("/api/v2/app/preferences", get(app::preferences_handler))
        .route(
            "/api/v2/torrents/info",
            get(torrents::info).post(torrents::info),
        )
        .route(
            "/api/v2/torrents/properties",
            get(torrents::properties).post(torrents::properties),
        )
        .route(
            "/api/v2/torrents/files",
            get(torrents::files).post(torrents::files),
        )
        .route("/api/v2/torrents/add", post(torrents::add))
        .route(
            "/api/v2/torrents/delete",
            post(torrents::delete).get(torrents::delete),
        )
        .route(
            "/api/v2/torrents/pause",
            post(torrents::pause).get(torrents::pause),
        )
        .route(
            "/api/v2/torrents/stop",
            post(torrents::pause).get(torrents::pause),
        )
        .route(
            "/api/v2/torrents/resume",
            post(torrents::resume).get(torrents::resume),
        )
        .route(
            "/api/v2/torrents/start",
            post(torrents::resume).get(torrents::resume),
        )
        .route(
            "/api/v2/torrents/categories",
            get(torrents::categories).post(torrents::categories),
        )
        .route("/api/v2/torrents/filePrio", post(torrents::file_priority))
        .route(
            "/api/v2/torrents/setShareLimits",
            post(torrents::set_share_limits),
        )
        .route("/api/v2/torrents/setCategory", post(torrents::set_category))
        .route(
            "/api/v2/torrents/createCategory",
            post(torrents::create_category),
        )
        // Everything else under the prefix is `404`, as qBittorrent answers an endpoint it
        // does not have. Without this the request fell through to the web interface and came
        // back as `200` with `index.html`, which a client parsing JSON reads as a server that
        // is up and broken rather than a feature that is missing. Guarded like the rest: an
        // unauthenticated probe learns nothing about which paths exist.
        .route("/api/v2/{*rest}", axum::routing::any(not_found))
}

async fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

pub(crate) fn routes(state: &AppState) -> Router<AppState> {
    public_routes().merge(
        guarded_routes().route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_api_token,
        )),
    )
}

/// Refuses a request that carries no valid `api:*` credential.
///
/// qBittorrent answers `403 Forbidden` with an empty body for an unauthenticated call, and
/// clients treat that as "log in again" rather than "the server is broken". That shape is
/// theirs and is kept.
async fn require_api_token(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    // The cookie is a handle first. Reading it as a token when it resolves to nothing keeps
    // working what clients already send: one configured by hand with its bearer in the `SID`
    // cookie, or one holding a cookie this adapter handed out before handles existed, still
    // authenticates. What changed is only what this service *hands out*.
    let credential = match cookie(request.headers()) {
        Some(sid) => state.qbittorrent_sessions.resolve(&sid).unwrap_or(sid),
        None => bearer(request.headers()).unwrap_or_default(),
    };
    if valid_token(&state, &credential).await {
        next.run(request).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// `POST /api/v2/auth/login`.
///
/// qBittorrent answers `Ok.` or `Fails.` as plain text with HTTP 200 either way; clients
/// compare the body, so a status code would not be read.
async fn login(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
    Query(query): Query<LoginForm>,
    body: String,
) -> Response {
    // Metered like `handlers::login`. Every attempt here costs an unauthenticated caller a
    // SHA-256 and a database lookup, and leaving this door free while the native one is
    // counted makes the counting on the native one decorative: an attacker guesses tokens
    // through whichever door does not answer back.
    if let rd_authn::Decision::Locked { .. } = state.auth.throttle_check(client.0).await {
        // qBittorrent's own answer for a banned address, which its clients already understand.
        return (
            StatusCode::FORBIDDEN,
            "Your IP address has been banned after too many failed login attempts.",
        )
            .into_response();
    }
    if let rd_authn::Decision::Proceed { delay } = state.auth.throttle_check(client.0).await
        && !delay.is_zero()
    {
        tokio::time::sleep(delay).await;
    }

    // Clients send the credentials as a form body; a few send them in the query string.
    // Parsed by hand rather than with a `Form` extractor so an empty or non-form body on a
    // `GET` login is simply "no password" instead of a rejection the client cannot read.
    let password = url::form_urlencoded::parse(body.as_bytes())
        .find(|(key, _)| key == "password")
        .map(|(_, value)| value.into_owned())
        .or(query.password)
        .or_else(|| bearer(&headers))
        .unwrap_or_default();
    if !valid_token(&state, &password).await {
        state.auth.note_failed_login(client.0).await;
        return "Fails.".into_response();
    }
    (
        [(header::SET_COOKIE, session_cookie(&state, &password).await)],
        "Ok.",
    )
        .into_response()
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // The handle dies here, not only in the browser: a cookie a client keeps after logging
    // out must be worth nothing, and clearing it client-side is a request, not a guarantee.
    if let Some(sid) = cookie(&headers) {
        state.qbittorrent_sessions.forget(&sid);
    }
    // Cleared at the path it was set at, or the browser keeps the one it already holds.
    // Deliberately without `Secure`, like `AuthService::EXPIRED_COOKIE`: clearing has to work
    // whatever the configuration says now, including after it changed under the cookie.
    let path = cookie_path(&state).await;
    (
        [(
            header::SET_COOKIE,
            format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path={path}; Max-Age=0"),
        )],
        "Ok.",
    )
        .into_response()
}

/// The `Set-Cookie` value handed to a client that logged in.
///
/// The value is an opaque handle, never the bearer. The same `Secure` and base-path policy as
/// the native session cookie in [`crate::auth`] still applies: a handle at `Path=/` is readable
/// by every path on the origin and travels in clear text over a plain-HTTP hop, and while a
/// handle is worth far less than a bearer it is worth a live session. `Secure` cannot be
/// decided from the request — a TLS-terminating proxy forwards plain HTTP — so it comes from
/// the proxy contract the operator configured.
async fn session_cookie(state: &AppState, token: &str) -> String {
    let session = state.qbittorrent_sessions.issue(token);
    let proxy = state.proxy.read().await;
    let path = if proxy.base_path().is_empty() {
        "/"
    } else {
        proxy.base_path()
    };
    let secure = if proxy.cookie_is_secure() {
        "; Secure"
    } else {
        ""
    };
    format!("{SESSION_COOKIE}={session}; HttpOnly; SameSite=Strict; Path={path}{secure}")
}

/// Where the session cookie lives. The mount point, or the origin root when there is none.
async fn cookie_path(state: &AppState) -> String {
    let proxy = state.proxy.read().await;
    if proxy.base_path().is_empty() {
        "/".to_owned()
    } else {
        proxy.base_path().to_owned()
    }
}

async fn valid_token(state: &AppState, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
    state
        .database
        .capture_token_valid(&digest, rd_core::API_SCOPE)
        .await
        .unwrap_or(false)
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
}

fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{SESSION_COOKIE}=")))
        .map(str::to_owned)
}

/// qBittorrent's plain acknowledgement for a successful action.
pub(crate) fn ok() -> Response {
    StatusCode::OK.into_response()
}
