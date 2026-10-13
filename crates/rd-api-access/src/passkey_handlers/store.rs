//! The passkey store behind the ceremonies in [`super`]: the installation's user handle, the
//! relying party, the enrolled passkeys and the small helpers the handlers share. Split from
//! `passkey_handlers.rs` so neither file outgrows the 500-line rule (RD-1280-08).

use axum::http::{HeaderMap, header};
use secrecy::ExposeSecret;
use webauthn_rs::prelude::{Passkey, Webauthn};

use crate::{ApiError, AppState};

/// Where this installation's WebAuthn user handle is kept.
///
/// Generated once and then stable: an authenticator groups credentials by it, so a handle that
/// changed between enrolments would show up as a second, unrelated account in the picker. Per
/// installation rather than a constant in the binary, so that two rDownloader users are not
/// correlatable by their passkey provider.
const USER_HANDLE_SETTING: &str = "mfa.webauthn_user_handle";

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
pub(super) async fn relying_party(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Webauthn, ApiError> {
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
pub(super) async fn load_passkeys(
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
pub(super) async fn user_handle(state: &AppState) -> Result<uuid::Uuid, ApiError> {
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

pub(super) fn clean_label(label: Option<String>, fallback: &str) -> String {
    label
        .map(|label| label.trim().chars().take(60).collect::<String>())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

pub(super) fn encoding_failed(error: serde_json::Error) -> ApiError {
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
