//! The password sign-in switch (RD-190-15, ADR 0021, decision D3).
//!
//! The password form goes off only from the session the latest sign-in through the identity
//! provider opened — proof that the round trip works — with an identity bound and the password
//! typed again. It comes back on only from the machine the service runs on:
//! `rdownloader auth password-login on`, which calls `POST /auth/password-login/on` with the local
//! control token, or, with the service stopped, writes the setting itself. No session and no API
//! token can turn it back on, so a provider that is down or deleted locks out nobody who can
//! reach the machine (O-LOCK). The password stays the step-up credential while the form is off.

use axum::{Json, extract::State, http::HeaderMap};
use rd_api_core::oidc_client;

use crate::{ApiError, AppState, audit::AuditContext, dto::MessageResponse};

/// Switches the password sign-in off (D3). Requires the session the latest provider sign-in
/// opened, and the password; only `rdownloader auth password-login on` turns it back on.
#[utoipa::path(
    post,
    path = "/api/v1/auth/password-login/off",
    tag = "security",
    request_body = crate::oidc_settings_handlers::OidcStepUpRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
        (status = 403, description = "Not a signed-in session", body = crate::error::ErrorBody),
        (status = 409, description = "This session was not opened through the provider", body = crate::error::ErrorBody),
    )
)]
pub async fn switch_password_login_off(
    State(state): State<AppState>,
    audit: AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
    Json(request): Json<crate::oidc_settings_handlers::OidcStepUpRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::step_up::require_step_up(
        &state,
        &audit,
        this_machine,
        client.0,
        &request.password,
        rd_core::AuditAction::PasswordLoginChanged,
    )
    .await?;
    let linked = match oidc_client::config(&state).await? {
        Some(config) => oidc_client::identity(&state, &config).await?,
        None => None,
    };
    let current = state
        .auth
        .current_session(&state, &headers)
        .await
        .map(|session| session.id.to_string());
    let proven = oidc_client::proven_session(&state).await?;
    // Proof that the round trip works: this very session came back from the provider.
    if linked.is_none() || current.is_none() || current != proven {
        return Err(ApiError::conflict(
            "auth.password_login_needs_provider_session",
            "Sign in through the identity provider first; the password sign-in can only be \
             switched off from that session",
        ));
    }
    state
        .database
        .set_setting(
            oidc_client::PASSWORD_LOGIN_OFF_SETTING.to_owned(),
            serde_json::Value::Bool(true),
        )
        .await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PasswordLoginChanged)
            .by(&audit)
            .client(client.0)
            .detail("enabled", false),
    )
    .await;
    Ok(Json(MessageResponse::new(
        "auth.password_login_switched_off",
        "Signing in with the password is switched off",
    )))
}

/// Switches the password sign-in back on (D3): `rdownloader auth password-login on`, with the
/// local control token, from this machine. Neither a session nor any API token may.
#[utoipa::path(
    post,
    path = "/api/v1/auth/password-login/on",
    tag = "security",
    responses(
        (status = 200, body = MessageResponse),
        (status = 403, description = "Not the command line on this machine", body = crate::error::ErrorBody),
    )
)]
pub async fn switch_password_login_on(
    State(state): State<AppState>,
    audit: AuditContext,
    crate::client::ThisMachine(this_machine): crate::client::ThisMachine,
) -> Result<Json<MessageResponse>, ApiError> {
    if !this_machine || audit.actor != crate::audit::Actor::local_control() {
        return Err(ApiError::forbidden(
            "auth.password_login_cli_only",
            "Only `rdownloader auth password-login on` on the machine the service runs on can \
             switch the password sign-in back on",
        ));
    }
    state
        .database
        .set_setting(
            oidc_client::PASSWORD_LOGIN_OFF_SETTING.to_owned(),
            serde_json::Value::Bool(false),
        )
        .await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PasswordLoginChanged)
            .by(&audit)
            .detail("enabled", true)
            .detail("via", "cli"),
    )
    .await;
    Ok(Json(MessageResponse::new(
        "auth.password_login_switched_on",
        "Signing in with the password is switched on again",
    )))
}

/// The refusal of anything that would end the provider sign-in while the password form is off.
pub(crate) fn password_login_is_off() -> ApiError {
    ApiError::conflict(
        "auth.oidc_password_login_off",
        "The password sign-in is switched off; switch it back on with `rdownloader auth \
         password-login on` on the machine the service runs on first",
    )
}
