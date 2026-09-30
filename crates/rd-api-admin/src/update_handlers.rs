//! The application update check over REST (RD-180-01), and the self-update it offers
//! (RD-180-02).
//!
//! Reading the status is `Read`, like the About page it sits beside: which version runs is no
//! secret from a status page. Checking is `Admin`: it makes the service reach out to GitHub on
//! the caller's word, and the channel it reads is the administrator's decision. Not audited,
//! like refreshing the tool manifest or a plugin repository: a check changes nothing but what
//! the status shows, and the channel it reads is a settings change, audited as one.

use axum::{Json, extract::State, http::StatusCode};

use crate::audit::AuditContext;
use crate::dto::{UpdateInstallRequest, UpdateInstallStatus, UpdateStatusResponse};
use crate::{AppState, error::ApiError};

#[utoipa::path(get, path = "/api/v1/system/update", tag = "system", responses((status = 200, body = UpdateStatusResponse)))]
pub async fn get_update_status(State(state): State<AppState>) -> Json<UpdateStatusResponse> {
    Json(state.updates.status().await)
}

/// Checks now and answers with the status. A manifest that was refused or could not be reached
/// is the status's `error_code`, not an error of this call: the check ran, and that is its
/// result. A build without the update signing key answers `409 update.not_configured`.
#[utoipa::path(
    post,
    path = "/api/v1/system/update/check",
    tag = "system",
    responses(
        (status = 200, body = UpdateStatusResponse),
        (status = 409, description = "This build carries no update signing key")
    )
)]
pub async fn check_for_updates(
    State(state): State<AppState>,
) -> Result<Json<UpdateStatusResponse>, ApiError> {
    match state.updates.check().await {
        Ok(_) => Ok(Json(state.updates.status().await)),
        Err(rd_update::UpdateError::NotConfigured) => Err(ApiError::conflict(
            "update.not_configured",
            "This build carries no update signing key; the update check is not configured",
        )),
        Err(error) => Err(anyhow::Error::new(error).into()),
    }
}

/// Installs the offered update and restarts (RD-180-02): answers at once with the step it begins
/// in, and `GET /api/v1/system/update` follows it through the restart. `Admin` and audited, like
/// stopping the service, which it does; no MCP tool, since it ends the session that would ask.
#[utoipa::path(
    post,
    path = "/api/v1/system/update/install",
    tag = "system",
    request_body = UpdateInstallRequest,
    responses(
        (status = 202, body = UpdateInstallStatus),
        (status = 409, body = crate::error::ErrorBody, description = "Nothing to install, not an installation that installs itself, downloads running, or the program folder is not fit")
    )
)]
pub async fn install_update(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<UpdateInstallRequest>,
) -> Result<(StatusCode, Json<UpdateInstallStatus>), ApiError> {
    let status = crate::update_install_service::start(&state, request, &audit).await?;
    Ok((StatusCode::ACCEPTED, Json(status)))
}
