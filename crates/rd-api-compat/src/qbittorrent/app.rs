//! The `app/*` endpoints: what a client reads before it trusts the server.

use axum::response::{IntoResponse, Response};

use crate::AppState;

/// Client version string. Clients gate features on it, so it names a release whose Web API
/// this subset matches.
pub(crate) const APP_VERSION: &str = "v4.6.5";
/// Web API version, which is what clients actually branch on.
pub(crate) const WEB_API_VERSION: &str = "2.9.3";

pub(crate) fn version() -> Response {
    APP_VERSION.into_response()
}

pub(crate) fn web_api_version() -> Response {
    WEB_API_VERSION.into_response()
}

/// The preference subset a client reads.
///
/// Only what a download client is asked for: where finished files land, and the queueing
/// switches a client checks before it decides whether a paused torrent will ever start.
/// Nothing about the network, proxies or credentials is exposed here.
pub(crate) fn preferences(state: &AppState) -> Response {
    let save_path = state
        .scheduler
        .downloads_directory()
        .to_string_lossy()
        .into_owned();
    axum::Json(serde_json::json!({
        "save_path": save_path,
        "temp_path": save_path,
        "temp_path_enabled": false,
        "create_subfolder_enabled": true,
        "start_paused_enabled": false,
        "auto_delete_mode": 0,
        "preallocate_all": true,
        "queueing_enabled": true,
        "max_active_downloads": 5,
        "max_active_torrents": 5,
        "max_active_uploads": 5,
        "dht": true,
        "pex": true,
        "lsd": true,
        "listen_port": 6881,
    }))
    .into_response()
}

/// Route handlers, now that authentication is a layer rather than a call inside each one.
pub(crate) async fn version_handler() -> Response {
    version()
}

pub(crate) async fn web_api_version_handler() -> Response {
    web_api_version()
}

pub(crate) async fn preferences_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Response {
    preferences(&state)
}
