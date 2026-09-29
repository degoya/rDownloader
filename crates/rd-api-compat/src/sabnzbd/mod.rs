//! A SABnzbd-compatible API subset for Usenet automation clients.
//!
//! Sonarr, Radarr, Lidarr, Readarr and similar tools talk to a download client over
//! SABnzbd's `mode=` query API. This module answers the modes those clients actually use and
//! nothing more; every other mode reports a compatible failure rather than pretending.
//!
//! Two conventions of that API differ from ours and are kept deliberately:
//! errors answer `HTTP 200` with `{"status": false, "error": …}`, because a SABnzbd client
//! reads the body and treats a non-200 as the server being down; and sizes are strings.

mod config;
mod history;
mod intake;
mod map;
mod queue;

use axum::{
    Router,
    extract::{Multipart, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::AppState;

/// Version reported to clients. A SABnzbd client refuses to talk to a server that claims a
/// version it does not know, so this names a release whose API this subset matches.
pub(crate) const REPORTED_VERSION: &str = "4.3.3";

/// Query parameters of the SABnzbd API.
///
/// Everything is optional and everything is a string: clients send `value`, `value2`, `name`
/// and `cat` with meanings that depend on `mode`, and an unknown extra parameter must never
/// make the request fail.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct SabQuery {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub cat: Option<String>,
    #[serde(default)]
    pub nzbname: Option<String>,
    /// The API key, when the client cannot send `X-Api-Key`.
    ///
    /// Kept because real SABnzbd clients send it here, and redacted from this service's request
    /// spans for the same reason it is second choice: a query string ends up in logs a header
    /// never touches.
    #[serde(default)]
    pub apikey: Option<String>,
}

/// The SABnzbd-compatible routes.
///
/// Mounted at `/sabnzbd/api` and at the bare `/api` that SABnzbd itself serves, so a client
/// configured with either base URL finds the endpoint. Neither collides with `/api/v1/*`:
/// both are exact paths.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api", get(handle).post(handle))
        .route("/sabnzbd/api", get(handle).post(handle))
}

async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SabQuery>,
    multipart: Option<Multipart>,
) -> Response {
    if !authorized(&state, &headers, query.apikey.as_deref()).await {
        return error("API Key Incorrect");
    }
    match query.mode.as_str() {
        "version" => config::version(),
        "auth" => config::auth(),
        "get_config" => config::get_config(&state).await,
        "get_cats" => config::get_cats(&state).await,
        "fullstatus" | "status" => config::status(&state).await,
        "queue" => queue::handle(&state, &query).await,
        "history" => history::handle(&state, &query).await,
        "addfile" => intake::add_file(&state, &query, multipart).await,
        "addurl" | "addlocalfile" => intake::add_url(&state, &query).await,
        "pause" => queue::pause_all(&state).await,
        "resume" => queue::resume_all(&state).await,
        other => error(&format!("not implemented: {other}")),
    }
}

/// Checks the API key against the revocable `api:*` tokens.
///
/// The same credential a machine client already uses, rather than a second secret: adding a
/// download client in Sonarr and adding an MCP client are the same act of handing out full
/// API access. A read-only token is refused here — this surface adds and deletes.
///
/// Send it as the `X-Api-Key` header wherever the client allows it. The `apikey=` query
/// parameter is accepted because that is what the SABnzbd clients in the field send and
/// dropping it would break every one of them, but a query string is written into reverse-proxy
/// access logs and browser history, which a header is not. This service redacts the parameter
/// from its own request spans (`rd_api::redact_uri`); nothing it does reaches a proxy's log.
async fn authorized(state: &AppState, headers: &HeaderMap, apikey: Option<&str>) -> bool {
    let key = apikey
        .map(str::to_owned)
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    if key.is_empty() {
        return false;
    }
    let digest = hex::encode(Sha256::digest(key.as_bytes()));
    state
        .database
        .capture_token_valid(&digest, rd_core::API_SCOPE)
        .await
        .unwrap_or(false)
}

/// A SABnzbd failure: `HTTP 200` with `status: false`.
pub(crate) fn error(message: &str) -> Response {
    axum::Json(serde_json::json!({ "status": false, "error": message })).into_response()
}

/// A SABnzbd success acknowledgement.
pub(crate) fn ok() -> Response {
    axum::Json(serde_json::json!({ "status": true })).into_response()
}

/// A SABnzbd success carrying a payload.
pub(crate) fn json(value: serde_json::Value) -> Response {
    axum::Json(value).into_response()
}
