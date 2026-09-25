//! Reading the reconnect state and asking for one by hand.

use axum::{Json, extract::State, http::StatusCode};

use crate::{AppState, error::ApiError, reconnect_service::ReconnectStatus};

#[utoipa::path(
    get,
    path = "/api/v1/reconnect",
    tag = "configuration",
    responses((status = 200, body = ReconnectStatus))
)]
pub async fn reconnect_status(
    State(state): State<AppState>,
) -> Result<Json<ReconnectStatus>, ApiError> {
    let settings = crate::handlers::read_settings(&state).await?;
    let status = state.reconnect.status(&state, &settings).await;
    Ok(Json(status))
}

/// Reconnects now.
///
/// Skips the window and the interval — somebody asked for it, and both of those exist to stop
/// the watcher acting on its own. The switch that decides whether running transfers may be
/// interrupted still applies, because that one is about losing work, not about timing.
#[utoipa::path(
    post,
    path = "/api/v1/reconnect",
    tag = "configuration",
    responses(
        (status = 202, body = crate::dto::MessageResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn trigger_reconnect(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<crate::dto::MessageResponse>), ApiError> {
    let settings = crate::handlers::read_settings(&state).await?;
    if !settings.reconnect_enabled {
        return Err(ApiError::bad_request(
            "reconnect.disabled",
            "Reconnecting is switched off",
        ));
    }
    if settings
        .reconnect_script
        .as_deref()
        .is_none_or(|name| name.trim().is_empty())
    {
        return Err(ApiError::bad_request(
            "reconnect.script_missing",
            "No reconnect script is configured",
        ));
    }
    if state.reconnect.busy().await {
        return Err(ApiError::conflict(
            "reconnect.already_running",
            "A reconnect is already running",
        ));
    }
    // Answered immediately: the attempt takes as long as the router does, which is longer
    // than a request should wait. The status endpoint and the event say how it went.
    let service = state.reconnect.clone();
    let background = state.clone();
    tokio::spawn(async move {
        service.run(&background, &settings).await;
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::dto::MessageResponse::new(
            "reconnect.started",
            "Reconnecting",
        )),
    ))
}
