//! Passkeys: enrolment from a signed-in session, and sign-in without a password.
//!
//! ## A passkey is an alternative, not a gate
//!
//! The authenticator app in [`crate::mfa_handlers`] is a *second* factor: it is demanded after
//! the password and it makes signing in strictly harder. A passkey is the other shape. The
//! ceremony here requires user verification, so the authenticator has already checked a PIN or
//! a fingerprint before it will sign — possession and knowledge, both, in one step. Demanding
//! a password as well would add nothing and cost the property that makes passkeys worth having.
//!
//! So enrolling a passkey does not turn on the second-factor prompt, and turning the second
//! factor off does not remove the passkeys. They are two independent ways in, and the password
//! remains a third. That is deliberate for a service with no support desk: the recurring
//! failure here is not "too easy to get in", it is "locked out of your own downloads".
//!
//! ## What is stored, and why it is rewritten on every sign-in
//!
//! A credential row holds a label and timestamps; the passkey itself — credential id, public
//! key, signature counter, backup flags — goes into the encrypted store as JSON, like every
//! other credential in this application. It is rewritten after a successful sign-in whenever
//! the authenticator reports a change, because a signature counter that is never persisted is
//! a counter that can never notice it went backwards, which is how a cloned hardware key is
//! detected at all.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use webauthn_rs::prelude::{Passkey, PublicKeyCredential, RegisterPublicKeyCredential, Webauthn};

use crate::{ApiError, AppState, dto::MessageResponse};

/// Where this installation's WebAuthn user handle is kept.
///
/// Generated once and then stable: an authenticator groups credentials by it, so a handle that
/// changed between enrolments would show up as a second, unrelated account in the picker. Per
/// installation rather than a constant in the binary, so that two rDownloader users are not
/// correlatable by their passkey provider.
const USER_HANDLE_SETTING: &str = "mfa.webauthn_user_handle";

/// The account name shown beside the passkey in an authenticator's list.
const ACCOUNT: &str = "administrator";

/// What starting a ceremony hands back.
#[derive(Debug, Serialize, ToSchema)]
pub struct PasskeyChallenge {
    /// Echoed back to finish the ceremony. Identifies the challenge, does not authorise it.
    pub ceremony_id: String,
    /// The `PublicKeyCredentialCreationOptions` or `…RequestOptions` to hand to the browser.
    ///
    /// Opaque here on purpose: this is a W3C-defined structure that the browser parses, and
    /// restating its shape in our own schema would only create a second definition to keep in
    /// step with the first.
    #[schema(value_type = Object)]
    pub options: serde_json::Value,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PasskeyConfirmRequest {
    pub ceremony_id: String,
    /// What to call this passkey in the list. Asked for after the ceremony, so nobody has to
    /// name a key before finding out whether their authenticator would produce one.
    #[serde(default)]
    pub label: Option<String>,
    /// The browser's `navigator.credentials.create()` result, serialised.
    #[schema(value_type = Object)]
    pub credential: serde_json::Value,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PasskeyLoginRequest {
    pub ceremony_id: String,
    /// The browser's `navigator.credentials.get()` result, serialised.
    #[schema(value_type = Object)]
    pub credential: serde_json::Value,
}

/// Starts enrolling a passkey. Requires a signed-in session.
#[utoipa::path(
    post,
    path = "/api/v1/mfa/passkey",
    tag = "security",
    responses(
        (status = 200, body = PasskeyChallenge),
        (status = 409, description = "No usable origin for passkeys", body = crate::error::ErrorBody),
    )
)]
pub async fn enrol_passkey(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
) -> Result<Json<PasskeyChallenge>, ApiError> {
    let webauthn = relying_party(&state, &headers).await?;
    let handle = user_handle(&state).await?;

    // Existing passkeys are excluded so the same authenticator cannot be enrolled twice. Two
    // rows backed by one device look like redundancy and are not: losing the device loses both.
    let existing = load_passkeys(&state).await?;
    let exclude = existing
        .iter()
        .map(|(_, passkey)| passkey.cred_id().clone())
        .collect::<Vec<_>>();

    let (options, registration) = webauthn
        .start_passkey_registration(handle, ACCOUNT, ACCOUNT, Some(exclude))
        .map_err(|error| {
            tracing::warn!(error = %error, "could not start passkey registration");
            ApiError::bad_request(
                "mfa.passkey_start_failed",
                "Could not start the passkey setup",
            )
        })?;

    // No row is written yet. A registration the user abandons — closes the dialog, has no
    // authenticator to hand — should leave nothing behind to clean up, and a row that exists
    // before the key does is a row whose `material_ref` points at nothing.
    let ceremony_id = state.passkey_registrations.insert(client.0, registration);
    Ok(Json(PasskeyChallenge {
        ceremony_id,
        options: serde_json::to_value(options).map_err(encoding_failed)?,
    }))
}

/// Finishes enrolling a passkey, storing it as a confirmed credential.
#[utoipa::path(
    post,
    path = "/api/v1/mfa/passkey/confirm",
    tag = "security",
    request_body = PasskeyConfirmRequest,
    responses(
        (status = 201, body = rd_core::MfaCredential),
        (status = 400, description = "The ceremony did not verify", body = crate::error::ErrorBody),
    )
)]
pub async fn confirm_passkey(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PasskeyConfirmRequest>,
) -> Result<(StatusCode, Json<rd_core::MfaCredential>), ApiError> {
    let webauthn = relying_party(&state, &headers).await?;
    let Some(registration) = state.passkey_registrations.take(&request.ceremony_id) else {
        return Err(ApiError::bad_request(
            "mfa.passkey_ceremony_expired",
            "That passkey setup has expired. Start it again.",
        ));
    };
    let credential: RegisterPublicKeyCredential = serde_json::from_value(request.credential)
        .map_err(|_| {
            ApiError::bad_request(
                "mfa.passkey_response_invalid",
                "The authenticator's response could not be read",
            )
        })?;
    let passkey = webauthn
        .finish_passkey_registration(&credential, &registration)
        .map_err(|error| {
            tracing::warn!(error = %error, "passkey registration did not verify");
            ApiError::bad_request("mfa.passkey_invalid", "That passkey could not be verified")
        })?;

    let material_ref = state
        .secrets
        .put_string(serde_json::to_string(&passkey).map_err(encoding_failed)?)
        .await?;
    let credential = state
        .database
        .create_mfa_credential(
            rd_core::MfaCredentialId::new(),
            rd_core::MfaKind::Webauthn,
            clean_label(request.label, "Passkey"),
            material_ref,
        )
        .await?;
    // Confirmed immediately, unlike the authenticator app. The two-step enrolment there exists
    // because scanning a QR code can silently fail; here the ceremony that just verified *is*
    // the proof the key works, so a separate confirmation would only be theatre.
    state.database.confirm_mfa_credential(credential.id).await?;
    let stored = state
        .database
        .list_mfa_credentials()
        .await?
        .into_iter()
        .find(|entry| entry.id == credential.id)
        .unwrap_or(credential);
    Ok((StatusCode::CREATED, Json(stored)))
}

/// Starts signing in with a passkey. Reachable without a session — it is how you sign in.
#[utoipa::path(
    post,
    path = "/api/v1/auth/passkey/challenge",
    tag = "auth",
    responses(
        (status = 200, body = PasskeyChallenge),
        (status = 404, description = "No passkey is enrolled", body = crate::error::ErrorBody),
        (status = 429, description = "Too many attempts from this address", body = crate::error::ErrorBody),
    )
)]
pub async fn passkey_challenge(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
) -> Result<Json<PasskeyChallenge>, ApiError> {
    // Metered like the sign-in it starts. This endpoint is public — it has to be — and it
    // allocates server state for an anonymous caller, so an address the limiter has already
    // locked out must not be able to keep starting ceremonies while it waits.
    if let rd_authn::Decision::Locked { retry_after } = state.auth.throttle_check(client.0).await {
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

    let webauthn = relying_party(&state, &headers).await?;
    let passkeys = load_passkeys(&state).await?;
    if passkeys.is_empty() {
        return Err(ApiError::not_found(
            "mfa.passkey_none_enrolled",
            "No passkey is set up for this installation",
        ));
    }
    let keys = passkeys
        .into_iter()
        .map(|(_, passkey)| passkey)
        .collect::<Vec<_>>();
    let (options, authentication) =
        webauthn
            .start_passkey_authentication(&keys)
            .map_err(|error| {
                tracing::warn!(error = %error, "could not start passkey authentication");
                ApiError::bad_request(
                    "mfa.passkey_start_failed",
                    "Could not start the passkey sign-in",
                )
            })?;
    // Filed against the caller's address. The ceiling on in-flight ceremonies is per address,
    // so a caller looping on this endpoint evicts only its own challenges — it used to push
    // out the oldest entry in the store, which was the one the legitimate user was in the
    // middle of answering.
    let ceremony_id = state
        .passkey_authentications
        .insert(client.0, authentication);
    Ok(Json(PasskeyChallenge {
        ceremony_id,
        options: serde_json::to_value(options).map_err(encoding_failed)?,
    }))
}

/// Finishes a passkey sign-in and opens a session.
#[utoipa::path(
    post,
    path = "/api/v1/auth/passkey/login",
    tag = "auth",
    request_body = PasskeyLoginRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 401, description = "The passkey did not verify", body = crate::error::ErrorBody),
    )
)]
pub async fn passkey_login(
    State(state): State<AppState>,
    client: crate::client::ClientAddress,
    headers: HeaderMap,
    Json(request): Json<PasskeyLoginRequest>,
) -> Result<Response, ApiError> {
    // The same throttle as the password login. A signature is far harder to guess than a
    // password, but this endpoint reaches the same session-opening code, and leaving one door
    // unmetered would make the metering on the other one decorative.
    if let rd_authn::Decision::Locked { retry_after } = state.auth.throttle_check(client.0).await {
        return Err(ApiError::too_many_requests(
            "auth.too_many_attempts",
            "Too many failed sign-in attempts from this address",
        )
        .with_param("seconds", retry_after.as_secs().max(1).to_string()));
    }
    if let rd_authn::Decision::Proceed { delay } = state.auth.throttle_check(client.0).await
        && !delay.is_zero()
    {
        // The other half of that symmetry. Honouring only the lockout left the distributed
        // case — every attempt from a fresh address, so the per-address counter never builds —
        // metered on the password login and free here, which is the door an attacker picks.
        tokio::time::sleep(delay).await;
    }

    let webauthn = relying_party(&state, &headers).await?;
    let Some(authentication) = state.passkey_authentications.take(&request.ceremony_id) else {
        return Err(ApiError::unauthorized(
            "mfa.passkey_ceremony_expired",
            "That sign-in attempt has expired. Try again.",
        ));
    };
    let credential: PublicKeyCredential =
        serde_json::from_value(request.credential).map_err(|_| {
            ApiError::unauthorized(
                "mfa.passkey_response_invalid",
                "The authenticator's response could not be read",
            )
        })?;
    let result = match webauthn.finish_passkey_authentication(&credential, &authentication) {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(error = %error, "passkey sign-in did not verify");
            state.auth.note_failed_login(client.0).await;
            return Err(crate::error_codes::invalid_credentials());
        }
    };

    // Which stored credential answered. A signature that verifies against a key we no longer
    // hold is not a sign-in: the row is what carries the label, the timestamps and the ability
    // to revoke it, so a passkey without one has already been removed.
    let Some((id, mut passkey)) = load_passkeys(&state)
        .await?
        .into_iter()
        .find(|(_, passkey)| passkey.cred_id() == result.cred_id())
    else {
        state.auth.note_failed_login(client.0).await;
        return Err(crate::error_codes::invalid_credentials());
    };

    if passkey.update_credential(&result) == Some(true) {
        // Best effort. Failing the sign-in because the counter could not be written back would
        // turn a storage hiccup into a lockout, and the counter is a clone *detector*, not the
        // thing that authenticated this request.
        match state
            .secrets
            .put_string(serde_json::to_string(&passkey).map_err(encoding_failed)?)
            .await
        {
            Ok(reference) => {
                if let Err(error) = state.database.repoint_mfa_material(id, reference).await {
                    tracing::warn!(error = %error, "could not store the passkey's updated state");
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "could not store the passkey's updated state");
            }
        }
    } else if let Err(error) = state.database.touch_mfa_credential(id).await {
        tracing::warn!(error = %error, "could not record the passkey's use");
    }

    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .and_then(rd_core::truncate_user_agent);
    let opened = state
        .auth
        .open_session(&state, client.0, user_agent)
        .await?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::LoginSucceeded)
            .actor(crate::audit::Actor::session(opened.id.to_string()))
            .client(client.0)
            .target("session", opened.id)
            .detail("method", "passkey"),
    )
    .await;
    let mut response = Json(MessageResponse::new("auth.logged_in", "Logged in")).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        axum::http::HeaderValue::from_str(&{
            let proxy = state.proxy.read().await;
            crate::AuthService::cookie(
                &opened.token,
                proxy.cookie_is_secure(),
                proxy.base_path(),
                opened.max_age_seconds,
            )
        })
        .map_err(|_| {
            ApiError::bad_request("auth.session_create_failed", "Session could not be created")
        })?,
    );
    Ok(response)
}

/// Whether any passkey is enrolled, for the sign-in screen.
pub(crate) async fn any_passkey_enrolled(state: &AppState) -> Result<bool, ApiError> {
    Ok(!state
        .database
        .confirmed_mfa_material(rd_core::MfaKind::Webauthn)
        .await?
        .is_empty())
}

/// Builds the relying party, translating "this installation has no usable origin" into an
/// answer that says what to change rather than a generic failure.
async fn relying_party(state: &AppState, headers: &HeaderMap) -> Result<Webauthn, ApiError> {
    let request_origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let proxy = state.proxy.read().await;
    rd_authn::relying_party(&proxy, request_origin)
        .map_err(|error| ApiError::conflict("mfa.passkey_origin_unknown", error.to_string()))
}

/// Every confirmed passkey, decoded.
///
/// A credential whose stored material will not decode is skipped with a warning rather than
/// failing the whole request: one unreadable row must not stop the other passkeys from working.
async fn load_passkeys(
    state: &AppState,
) -> Result<Vec<(rd_core::MfaCredentialId, Passkey)>, ApiError> {
    let mut passkeys = Vec::new();
    for (id, reference) in state
        .database
        .confirmed_mfa_material(rd_core::MfaKind::Webauthn)
        .await?
    {
        let stored = state.secrets.get(&reference).await?;
        match serde_json::from_str::<Passkey>(stored.expose_secret()) {
            Ok(passkey) => passkeys.push((id, passkey)),
            Err(error) => {
                tracing::warn!(error = %error, credential = %id, "a stored passkey is not readable");
            }
        }
    }
    Ok(passkeys)
}

/// Reads this installation's WebAuthn user handle, creating it on first use.
async fn user_handle(state: &AppState) -> Result<uuid::Uuid, ApiError> {
    if let Some(value) = state.database.get_setting(USER_HANDLE_SETTING).await?
        && let Some(handle) = value.as_str().and_then(|raw| raw.parse().ok())
    {
        return Ok(handle);
    }
    let handle = uuid::Uuid::new_v4();
    state
        .database
        .set_setting(
            USER_HANDLE_SETTING.to_owned(),
            serde_json::Value::String(handle.to_string()),
        )
        .await?;
    Ok(handle)
}

fn clean_label(label: Option<String>, fallback: &str) -> String {
    label
        .map(|label| label.trim().chars().take(60).collect::<String>())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn encoding_failed(error: serde_json::Error) -> ApiError {
    tracing::error!(error = %error, "could not encode a passkey ceremony");
    ApiError::bad_request(
        "mfa.passkey_start_failed",
        "Could not start the passkey setup",
    )
}

#[cfg(test)]
mod tests {
    use super::clean_label;

    #[test]
    fn a_label_falls_back_rather_than_being_empty() {
        assert_eq!(clean_label(None, "Passkey"), "Passkey");
        assert_eq!(clean_label(Some("   ".to_owned()), "Passkey"), "Passkey");
        assert_eq!(
            clean_label(Some(" Yubikey ".to_owned()), "Passkey"),
            "Yubikey"
        );
    }

    #[test]
    fn a_label_cannot_be_used_to_store_an_essay() {
        let long = "x".repeat(500);
        assert_eq!(clean_label(Some(long), "Passkey").chars().count(), 60);
    }
}
