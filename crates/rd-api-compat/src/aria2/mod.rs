//! An aria2 JSON-RPC subset for download front ends (RD-1240-11).
//!
//! AriaNg, Motrix-style interfaces, browser extensions that hand their downloads over and the
//! Android remotes all talk to aria2 over JSON-RPC 2.0 at `/jsonrpc`. This module answers the
//! calls they need to add a link and show the queue -- `addUri`, the `tell*` family, `pause`,
//! `unpause`, `remove`, `getGlobalStat`, `getVersion`, `system.listMethods` and the
//! `system.multicall` they batch with -- and the ones AriaNg's stopped list, task page and
//! settings pages send: `removeDownloadResult`, `purgeDownloadResult`, `getFiles`, `getUris`,
//! `getPeers`, `getServers`, `getOption`, `getGlobalOption`, and `changeOption` and
//! `changeGlobalOption` without effect (RD-1240-28). Every other method is JSON-RPC's `Method
//! not found` (`-32601`). No WebSocket transport, no `addTorrent` or `addMetalink` (owner,
//! 2026-10-10).
//!
//! Off unless `aria2_rpc_enabled` is set: switched off, `/jsonrpc` answers `404` to every
//! method, the preflight included, as a path that does not exist. The secret is aria2's
//! `token:<secret>` first parameter, and the secret is one of the revocable API tokens holding
//! `api:intake`, `api:queue` and `api:read` -- the same check as the SABnzbd and qBittorrent
//! adapters (`auth::compat_access`), its use recorded the same way.
//!
//! Open to every origin, as aria2 is with `--rpc-allow-origin-all`: AriaNg is a page served from
//! anywhere, and without the CORS answer the browser withholds every reply from it. Nothing
//! ambient rides along -- the secret is in the body, never in a cookie -- so a foreign page gets
//! nothing it does not already hold. The body is read before the secret is known, so it is
//! bounded like the native public routes.

mod methods;
mod options;
mod rpc;
mod status;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use tower_http::cors::CorsLayer;

use crate::AppState;

/// Version reported by `aria2.getVersion`: a release whose RPC this subset follows.
pub(crate) const REPORTED_VERSION: &str = "1.37.0";

pub(crate) fn routes(state: &AppState) -> Router<AppState> {
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE]);
    Router::new()
        .route("/jsonrpc", post(handle))
        .layer(axum::extract::DefaultBodyLimit::max(
            rd_api_core::container_upload::PUBLIC_BODY_LIMIT_BYTES,
        ))
        .layer(cors)
        // Outermost, so a switched-off adapter answers nothing else, its preflight included.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_enabled,
        ))
}

/// `404` unless the adapter is switched on in the settings.
async fn require_enabled(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    match rd_api_core::settings_store::read_settings(&state).await {
        Ok(settings) if settings.aria2_rpc_enabled => next.run(request).await,
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::warn!(
                error = error.message(),
                "the aria2 adapter could not read the settings"
            );
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// `POST /jsonrpc`: one call or a batch, every call carrying the same secret.
async fn handle(State(state): State<AppState>, body: Bytes) -> Response {
    let request = match rpc::parse(&body) {
        Ok(request) => request,
        Err(malformed) => return rpc::single(malformed.id, &Err(malformed.error)),
    };
    let Some(token) = rpc::request_token(&request) else {
        return rpc::unauthorized(&request);
    };
    match rd_api_core::auth::compat_access(&state, &token).await {
        rd_api_core::auth::CompatAccess::Granted => methods::answer(&state, request).await,
        rd_api_core::auth::CompatAccess::Refused => rpc::unauthorized(&request),
        // Not "Unauthorized": a front end told its secret is wrong asks for a new one, for a
        // fault that is this service's (as the other adapters, audit 1.9.1, API-13).
        rd_api_core::auth::CompatAccess::Unavailable => {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
        rd_api_core::auth::CompatAccess::RateLimited {
            retry_after_seconds,
        } => crate::rate_limited(retry_after_seconds),
    }
}
