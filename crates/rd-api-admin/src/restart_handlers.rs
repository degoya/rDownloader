//! A pending restart over REST (RD-1240-32): whether one is pending and why, and "restart now".
//!
//! Both are `Admin`: the status names the plugins that wait for the next start, which the plugin
//! list prices at the administrator's, and restarting stops the service, audited like the stop
//! (`restart_service`). The tray reads only whether a restart is pending and restarts through its
//! own routes with the agent's `capture:server_update` right (`capture_server_update`).

use axum::{Json, extract::State, http::StatusCode};

use crate::audit::AuditContext;
use crate::dto::{RestartRequest, RestartStartedResponse, RestartStatusResponse};
use crate::{AppState, error::ApiError};

#[utoipa::path(get, path = "/api/v1/system/restart", tag = "system", responses((status = 200, body = RestartStatusResponse)))]
pub async fn get_restart_status(State(state): State<AppState>) -> Json<RestartStatusResponse> {
    Json(crate::restart_service::status(&state).await)
}

/// Restarts the service now: answers at once with how it comes back, then stops. Running
/// downloads refuse it with `restart.transfers_active` unless `allow_active` is sent; the stop
/// saves them and they continue after the restart. `GET /api/v1/system/restart` answers with a
/// new `started_at` once the service is back.
#[utoipa::path(
    post,
    path = "/api/v1/system/restart",
    tag = "system",
    request_body = RestartRequest,
    responses(
        (status = 202, body = RestartStartedResponse),
        (status = 409, body = crate::error::ErrorBody, description = "An update is being installed, a restart runs already, downloads are running, or the relauncher could not be started")
    )
)]
pub async fn restart_service(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<RestartRequest>,
) -> Result<(StatusCode, Json<RestartStartedResponse>), ApiError> {
    let started = crate::restart_service::begin(&state, request, &audit, false).await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
}
