//! Changing the administrator password (RD-120-22).
//!
//! Until this existed there was no supported way to replace it at all: `setup()` wrote the
//! setting once and refused every later call, so a password that had ended up in a note, a
//! chat or a screenshot could only be removed by editing the SQLite file by hand.
//!
//! This is a **second door, not a widened first one**. `setup()` still refuses its second
//! call, and nothing here works without the current password. Recovery *without* it is a
//! separate decision with its own traps -- who may trigger it, what proves the right to, what
//! happens to the secrets in `rd-secrets` -- and is deliberately absent.
//!
//! Three properties are the whole point, and each is load-bearing:
//!
//! * **A wrong current password is indistinguishable from a wrong sign-in.** Same status, same
//!   code, same body, same limiter, same global delay. An attacker who could tell the two
//!   apart would have found a password oracle behind a route that a machine token can reach.
//! * **The new password's policy is judged before the current one is consulted.** The other
//!   order looks more natural and is the oracle: `auth.password_too_short` coming back only
//!   when the current password was *right* would answer the question the refusal is there to
//!   refuse.
//! * **Every session ends.** A password change that leaves the sessions opened with the old
//!   password alive protects against nothing. The caller's own session ends with them and is
//!   immediately replaced, so the change does not sign you out of the screen you made it on.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};

use crate::{ApiError, AppState, dto::MessageResponse, dto::PasswordChangeRequest};

#[utoipa::path(
    post,
    path = "/api/v1/auth/password",
    tag = "security",
    request_body = PasswordChangeRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 400, description = "The new password does not meet the policy, or is the current one", body = crate::error::ErrorBody),
        (status = 401, description = "The current password did not match", body = crate::error::ErrorBody),
        (status = 429, description = "Too many failed attempts from this address", body = crate::error::ErrorBody),
    )
)]
pub async fn change_password(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    audit: crate::audit::AuditContext,
    headers: HeaderMap,
    Json(request): Json<PasswordChangeRequest>,
) -> Result<Response, ApiError> {
    // The sign-in limiter, used rather than rebuilt: `rd_authn::throttle` counts per address
    // with a bounded, expiring lockout and a global delay that can never become a refusal,
    // which is what stops the protection from turning into the attack. A counter of its own
    // here would be a second, weaker copy of that reasoning.
    if let rd_authn::Decision::Locked { retry_after } = state.auth.throttle_check(client.0).await {
        note_refusal(&state, &audit, client.0, "locked_out").await;
        return Err(ApiError::too_many_requests(
            "auth.too_many_attempts",
            "Too many failed sign-in attempts from this address",
        )
        .with_param("seconds", retry_after.as_secs().max(1).to_string()));
    }
    if let rd_authn::Decision::Proceed { delay } = state.auth.throttle_check(client.0).await
        && !delay.is_zero()
    {
        tokio::time::sleep(delay).await;
    }

    // Before the current password is consulted, and on purpose: this refusal depends only on
    // what the caller sent, so it tells them nothing they did not already know. Checking the
    // current password first would make `auth.password_too_short` mean "and your old one was
    // right", which is the oracle this route must not be.
    crate::auth::validate_password(&request.new_password)?;

    let current_ok = state
        .auth
        .password_matches(&state, &request.current_password)
        .await?;
    if !current_ok {
        // Exactly the sign-in's refusal, counted in exactly the sign-in's limiter.
        state.auth.note_failed_login(client.0).await;
        note_refusal(&state, &audit, client.0, "current_password").await;
        return Err(crate::error_codes::invalid_credentials());
    }

    // Only now, with the current password already proved, so this answer reveals nothing.
    // Worth refusing: the reason to be on this screen at all is usually that the password in
    // force is the one that leaked, and "changed" without a change is the worst outcome.
    if state
        .auth
        .password_matches(&state, &request.new_password)
        .await?
    {
        return Err(ApiError::bad_request(
            "auth.password_unchanged",
            "The new password must differ from the current one",
        ));
    }

    state
        .auth
        .store_password(&state, &request.new_password)
        .await?;

    // Every session, the caller's own included. Anything opened with the old password is
    // exactly what the change is meant to end.
    let ended = state.database.revoke_all_sessions().await?;

    // ...and then straight back in, for the caller only, if they came with a session at all.
    // A machine token that called this has none, and is not handed one.
    let reopened = if crate::auth::session_digest(&headers).is_some() {
        let user_agent = headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .and_then(rd_core::truncate_user_agent);
        Some(
            state
                .auth
                .open_session(&state, client.0, user_agent)
                .await?,
        )
    } else {
        None
    };

    // Neither password reaches the record, and there is no field on `AuditEvent` that could
    // carry one. What it says is who changed it, from where, and how much it cost in sessions.
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::PasswordChanged)
        .by(&audit)
        .client(client.0)
        .detail("sessions_ended", ended);
    if let Some(opened) = reopened.as_ref() {
        event = event.target("session", opened.id);
    }
    crate::audit::record(&state, event).await;

    let mut response = Json(
        MessageResponse::new("auth.password_changed", "Password changed")
            .with_param("sessions_ended", ended.to_string()),
    )
    .into_response();
    if let Some(opened) = reopened.as_ref() {
        let cookie = {
            let proxy = state.proxy.read().await;
            crate::AuthService::cookie(
                &opened.token,
                proxy.cookie_is_secure(),
                proxy.base_path(),
                opened.max_age_seconds,
            )
        };
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).map_err(|_| {
                ApiError::bad_request("auth.session_create_failed", "Session could not be created")
            })?,
        );
    }
    Ok(response)
}

/// One refused change, recorded.
///
/// `stage` names what refused -- "current_password", "locked_out" -- never the value that was
/// wrong. An audit log of refused changes that quoted the attempts would be a dictionary of
/// near-miss passwords, which is the one thing it must not become.
async fn note_refusal(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    client: std::net::IpAddr,
    stage: &str,
) {
    crate::audit::record(
        state,
        crate::audit::AuditEvent::failure(rd_core::AuditAction::PasswordChanged)
            .by(audit)
            .client(client)
            .detail("stage", stage),
    )
    .await;
}
