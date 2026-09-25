//! Quiet-hours and completion-action status, plus the cancel action for a running power
//! countdown (RD-050-13).

use axum::{Json, extract::State};

use crate::{AppState, error::ApiError};

#[utoipa::path(get, path = "/api/v1/power/status", tag = "system", responses((status = 200, body = rd_power::PowerStatus)))]
pub async fn power_status(State(state): State<AppState>) -> Json<rd_power::PowerStatus> {
    Json(state.power.status(chrono::Utc::now()).await)
}

/// Aborts a counting-down standby or shutdown. The cycle counts as handled, so it does not
/// start counting again a second later.
#[utoipa::path(post, path = "/api/v1/power/cancel", tag = "system", responses((status = 200, body = crate::dto::MessageResponse)))]
pub async fn cancel_power_action(
    State(state): State<AppState>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    Ok(Json(if state.power.cancel().await {
        crate::dto::MessageResponse::new("power.cancelled", "Power action cancelled")
    } else {
        crate::dto::MessageResponse::new("power.nothing_pending", "No power action was pending")
    }))
}
