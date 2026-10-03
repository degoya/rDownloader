//! A new administrator password without the current one, from the machine the service runs on
//! (RD-190-24).
//!
//! `rdownloader auth reset-password` calls `POST /auth/password/reset` with the local control
//! token while the service runs, and writes the database itself while it is stopped; the
//! reasoning and the shared steps are in `rd_api_core::password_reset`. This route is the first
//! way: no session, no API token and no other machine may take it, so a forgotten password is
//! recovered by whoever holds the data directory and by nobody who merely reaches the service.

use axum::{Json, extract::State};
use rd_api_core::password_reset;

use crate::{ApiError, AppState, audit::AuditContext, dto::MessageResponse};

/// Sets a new administrator password without the current one: `rdownloader auth
/// reset-password`, with the local control token, from this machine. Neither a session nor any
/// API token may.
#[utoipa::path(
    post,
    path = "/api/v1/auth/password/reset",
    tag = "security",
    request_body = crate::dto::PasswordResetRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 400, description = "The new password does not meet the policy", body = crate::error::ErrorBody),
        (status = 403, description = "Not the command line on this machine", body = crate::error::ErrorBody),
        (status = 409, description = "No administrator password was set yet", body = crate::error::ErrorBody),
    )
)]
pub async fn reset_password_locally(
    State(state): State<AppState>,
    audit: AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
    Json(request): Json<crate::dto::PasswordResetRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    if !this_machine || audit.actor != crate::audit::Actor::local_control() {
        return Err(ApiError::forbidden(
            "auth.password_reset_cli_only",
            "Only `rdownloader auth reset-password` on the machine the service runs on can set \
             a new password without the current one",
        ));
    }
    let reset =
        password_reset::reset(&state.database, &request.new_password, request.disable_totp).await?;
    for reference in &reset.removed_material {
        if let Err(error) = state.secrets.remove(reference).await {
            tracing::warn!(error = %error, "could not remove a second factor's stored secret");
        }
    }
    // The owner has usually locked their own address out by now; a new password they cannot
    // try yet is no way back in.
    state.auth.reset_throttle().await;
    crate::audit::record(
        &state,
        password_reset::audit_event(
            &reset,
            password_reset::Path::Service,
            request.prompted,
            request.disable_totp,
        )
        .by(&audit),
    )
    .await;
    Ok(Json(
        MessageResponse::new(
            "auth.password_reset_done",
            "The administrator password was reset",
        )
        .with_param("sessions_ended", reset.sessions_ended)
        .with_param(
            "password_login",
            if reset.password_login_off {
                "off"
            } else {
                "on"
            },
        ),
    ))
}
