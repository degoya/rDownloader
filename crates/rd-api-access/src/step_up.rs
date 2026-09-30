//! Who may change how somebody signs in (security audit 2026-09-30, finding 2).
//!
//! Adding a passkey is adding a way in that needs no password, and adding or removing a second
//! factor changes what a sign-in asks for. The routes that do it cost `api:secrets`, so a token
//! holding the credentials area could enrol a passkey of its own and sign in as the
//! administrator with it — a full session out of a machine credential. Two rules close that:
//!
//! * **An interactive administrator only.** A signed-in session, or a caller on this machine
//!   while the login is switched off — never a bearer token, whatever its areas.
//! * **The password, again,** for everything that adds or removes a factor. A session somebody
//!   walked away from is a session, and the password is what it cannot give away. Checked
//!   exactly like the password change checks the current one: the sign-in limiter, the same
//!   refusal as a wrong sign-in, and a failure record that never quotes the attempt.

use crate::{ApiError, AppState, audit::AuditContext};

/// Refuses anybody but an interactive administrator.
///
/// `this_machine` is `client::ThisMachine` of the request: with the login switched off the
/// middleware attributes every local call to nobody, a signed-in browser included, and that
/// caller is the administrator the switch trusts.
pub(crate) fn require_interactive(
    state: &AppState,
    audit: &AuditContext,
    this_machine: bool,
) -> Result<(), ApiError> {
    if audit.actor.kind == rd_core::AuditActorKind::Session || state.auth.disabled_for(this_machine)
    {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "mfa.session_required",
        "Second factors and passkeys can only be changed from a signed-in session",
    ))
}

/// Refuses anybody but an interactive administrator who has just typed the password.
///
/// `action` is what a refusal is recorded as: the enrolment or the removal it stopped.
pub(crate) async fn require_step_up(
    state: &AppState,
    audit: &AuditContext,
    this_machine: bool,
    client: std::net::IpAddr,
    password: &str,
    action: rd_core::AuditAction,
) -> Result<(), ApiError> {
    if let Err(refusal) = require_interactive(state, audit, this_machine) {
        note_refusal(state, audit, client, action, "session").await;
        return Err(refusal);
    }
    if let rd_authn::Decision::Locked { retry_after } = state.auth.throttle_check(client).await {
        note_refusal(state, audit, client, action, "locked_out").await;
        return Err(ApiError::too_many_requests(
            "auth.too_many_attempts",
            "Too many failed sign-in attempts from this address",
        )
        .with_param("seconds", retry_after.as_secs().max(1).to_string()));
    }
    if let rd_authn::Decision::Proceed { delay } = state.auth.throttle_check(client).await
        && !delay.is_zero()
    {
        tokio::time::sleep(delay).await;
    }
    if !state.auth.password_matches(state, password).await? {
        state.auth.note_failed_login(client).await;
        note_refusal(state, audit, client, action, "password").await;
        return Err(crate::error_codes::invalid_credentials());
    }
    Ok(())
}

/// One refused change, recorded by the stage that refused it and never by what was typed.
async fn note_refusal(
    state: &AppState,
    audit: &AuditContext,
    client: std::net::IpAddr,
    action: rd_core::AuditAction,
    stage: &str,
) {
    crate::audit::record(
        state,
        crate::audit::AuditEvent::failure(action)
            .by(audit)
            .client(client)
            .detail("stage", stage),
    )
    .await;
}
