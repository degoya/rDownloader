//! Stopping the service and the backup before an update over the API (RD-180-02, RD-180-03):
//! what the launchers, `rdownloader stop` and the updater call on this machine.
//!
//! Both routes cost `api:admin` and are refused (`system.local_only`) for any request that did
//! not come straight from this machine — a loopback peer and no forwarding header, the same
//! test a switched-off login trusts (`client::from_this_machine`). Besides a session or an
//! `api:admin` token they accept the local control token (`local_control`), which opens these
//! two routes and no other. Neither has an MCP tool: stopping the service would end the session
//! that asked, and the backup before an update is the first step of a version switch no agent
//! performs (`mcp_coverage`).

use axum::{Json, extract::State, http::StatusCode};
use rd_core::AuditAction;
use serde::Serialize;
use utoipa::ToSchema;

use crate::audit::{AuditContext, AuditEvent};
use crate::client::ThisMachine;
use crate::pre_update_service::{PreUpdateRequest, PreUpdateResponse};
use crate::{ApiError, AppState};

/// The answer to a stop request.
#[derive(Serialize, ToSchema)]
pub struct ServiceStopResponse {
    /// Always `true`: the listener stops accepting now, the queue is checkpointed, then the
    /// process ends and removes its local control file.
    pub stopping: bool,
}

fn local_only(this_machine: ThisMachine) -> Result<(), ApiError> {
    if this_machine.0 {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "system.local_only",
        "This action is only available from the machine the service runs on",
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/shutdown",
    tag = "system",
    responses((status = 202, body = ServiceStopResponse), (status = 403, body = crate::error::ErrorBody))
)]
pub async fn shutdown_service(
    State(state): State<AppState>,
    this_machine: ThisMachine,
    audit: AuditContext,
) -> Result<(StatusCode, Json<ServiceStopResponse>), ApiError> {
    local_only(this_machine)?;
    crate::audit::record(
        &state,
        AuditEvent::success(AuditAction::ServiceStopRequested)
            .by(&audit)
            .target("service", "rdownloader"),
    )
    .await;
    tracing::info!("a graceful stop was requested over the API");
    // The graceful shutdown lets this request finish before the listener closes.
    state.shutdown.cancel();
    Ok((
        StatusCode::ACCEPTED,
        Json(ServiceStopResponse { stopping: true }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/system/update/prepare",
    tag = "system",
    request_body = PreUpdateRequest,
    responses(
        (status = 200, body = PreUpdateResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn prepare_update_backup(
    State(state): State<AppState>,
    this_machine: ThisMachine,
    audit: AuditContext,
    Json(request): Json<PreUpdateRequest>,
) -> Result<Json<PreUpdateResponse>, ApiError> {
    local_only(this_machine)?;
    Ok(Json(
        crate::pre_update_service::prepare(&state, request, &audit).await?,
    ))
}
