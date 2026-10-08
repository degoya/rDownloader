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
//!
//! Here, below the areas, since a full restore and the settings import ask for the same
//! (RD-1190-19): both replace the password hash, the passkeys and the tokens with whatever the
//! archive or the bundle holds, which made them a way in for a token as well
//! ([`require_confirmed`]).

use crate::{ApiError, AppState, audit::AuditContext};

/// Refuses anybody but an interactive administrator.
///
/// `this_machine` is `client::ThisMachine` of the request: with the login switched off the
/// middleware attributes every local call to nobody, a signed-in browser included, and that
/// caller is the administrator the switch trusts.
pub fn require_interactive(
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
pub async fn require_step_up(
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
    confirm_password(state, audit, client, password, action).await
}

/// Refuses a whole-installation replacement — a full restore, the settings import — to anybody
/// but an interactive administrator who has just typed the password (RD-1190-19).
///
/// The same two rules as [`require_step_up`], with its own refusal code, since the action is no
/// second factor. With the login switched off for this caller there is no password to ask
/// for: the switch already trusts whoever sits at this machine, and a request from elsewhere
/// is never let in by it.
pub async fn require_confirmed(
    state: &AppState,
    audit: &AuditContext,
    this_machine: bool,
    client: std::net::IpAddr,
    password: Option<&str>,
    action: rd_core::AuditAction,
) -> Result<(), ApiError> {
    if state.auth.disabled_for(this_machine) {
        return Ok(());
    }
    if audit.actor.kind != rd_core::AuditActorKind::Session {
        note_refusal(state, audit, client, action, "session").await;
        return Err(ApiError::forbidden(
            "auth.step_up_session_required",
            "Only a signed-in session can do this, with the password typed again",
        ));
    }
    confirm_password(state, audit, client, password.unwrap_or_default(), action).await
}

/// The password, checked like a sign-in: the limiter first, a miss counted against it.
async fn confirm_password(
    state: &AppState,
    audit: &AuditContext,
    client: std::net::IpAddr,
    password: &str,
    action: rd_core::AuditAction,
) -> Result<(), ApiError> {
    let _attempt = match state.auth.gate(client).await {
        Ok(attempt) => attempt,
        Err(refusal) => {
            note_refusal(state, audit, client, action, "locked_out").await;
            return Err(refusal);
        }
    };
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
