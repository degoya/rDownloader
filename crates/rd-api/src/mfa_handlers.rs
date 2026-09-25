//! Optional second factor: enrolment, confirmation, and the codes that get you back in.
//!
//! ## The rule that shapes all of it
//!
//! This is a self-hosted service with one account and nobody to appeal to. There is no support
//! desk that can verify who you are, so every path here is designed around the question "what
//! happens when the phone is gone" rather than around making the factor as strict as possible.
//! Three consequences:
//!
//! * Enrolment is two steps. A credential does not gate sign-in until a code from it has been
//!   accepted once — otherwise someone who scans the QR badly, or scans it into an app they
//!   then delete, is locked out by the act of trying to be safer.
//! * Recovery codes are issued with the first factor, not offered afterwards.
//! * Turning the factor off requires the current password, which is the credential that was
//!   being protected. Requiring a *code* to switch it off would make a lost phone permanent.

use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState, dto::MessageResponse};

/// The issuer shown in an authenticator app's list.
const ISSUER: &str = "rDownloader";
/// The account name shown beside it. One account, so it is a constant.
const ACCOUNT: &str = "administrator";

/// What enrolment hands back: the thing to scan, and the thing to write down.
#[derive(Debug, Serialize, ToSchema)]
pub struct TotpEnrolment {
    pub credential_id: rd_core::MfaCredentialId,
    /// `otpauth://` URL for the QR code.
    pub provisioning_uri: String,
    /// The same secret in base32, for an app that cannot scan.
    pub secret: String,
    /// Shown once. There is no second chance to read these.
    pub recovery_codes: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct TotpEnrolRequest {
    /// What to call this factor in the list.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MfaCodeRequest {
    pub code: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct MfaDisableRequest {
    /// The administrator password. Deliberately not a code: a lost phone must not be permanent.
    #[schema(write_only)]
    pub password: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/mfa",
    tag = "security",
    responses((status = 200, body = rd_core::MfaStatus))
)]
pub async fn mfa_status(
    State(state): State<AppState>,
) -> Result<Json<rd_core::MfaStatus>, ApiError> {
    Ok(Json(status_of(&state).await?))
}

pub(crate) async fn status_of(state: &AppState) -> Result<rd_core::MfaStatus, ApiError> {
    let credentials = state.database.list_mfa_credentials().await?;
    let remaining = state.database.unused_recovery_digests().await?.len();
    Ok(rd_core::MfaStatus {
        // Only the authenticator app. `enabled` answers "does signing in take a second step",
        // and a passkey is a different way to take the first one — see `passkey_handlers`.
        enabled: credentials.iter().any(|credential| {
            credential.kind == rd_core::MfaKind::Totp && credential.confirmed_at.is_some()
        }),
        credentials,
        recovery_codes_remaining: u32::try_from(remaining).unwrap_or(u32::MAX),
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/mfa/totp",
    tag = "security",
    request_body = TotpEnrolRequest,
    responses((status = 201, body = TotpEnrolment))
)]
pub async fn enrol_totp(
    State(state): State<AppState>,
    Json(request): Json<TotpEnrolRequest>,
) -> Result<(axum::http::StatusCode, Json<TotpEnrolment>), ApiError> {
    let secret = rd_authn::totp::generate_secret();
    // Into the encrypted store, not the database: the seed is a credential, and the same rule
    // that keeps account passwords out of SQLite applies to it.
    let material_ref = state
        .secrets
        .put_string(rd_authn::totp::base32_encode(&secret))
        .await?;
    let label = request
        .label
        .map(|label| label.trim().chars().take(60).collect::<String>())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "Authenticator app".to_owned());
    let credential = state
        .database
        .create_mfa_credential(
            rd_core::MfaCredentialId::new(),
            rd_core::MfaKind::Totp,
            label,
            material_ref,
        )
        .await?;

    // Issued now rather than after confirmation: the moment somebody is looking at a QR code
    // is the only moment they are also willing to write ten codes down.
    let codes = rd_authn::recovery::generate_codes();
    state
        .database
        .replace_recovery_codes(codes.iter().map(|code| code.digest.clone()).collect())
        .await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(TotpEnrolment {
            credential_id: credential.id,
            provisioning_uri: rd_authn::totp::provisioning_uri(&secret, ACCOUNT, ISSUER),
            secret: rd_authn::totp::base32_encode(&secret),
            recovery_codes: codes.into_iter().map(|code| code.plaintext).collect(),
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/mfa/totp/{id}/confirm",
    tag = "security",
    params(("id" = String, Path,)),
    request_body = MfaCodeRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 400, description = "The code did not match", body = crate::error::ErrorBody),
    )
)]
pub async fn confirm_totp(
    State(state): State<AppState>,
    Path(id): Path<rd_core::MfaCredentialId>,
    Json(request): Json<MfaCodeRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let Some(reference) = state.database.mfa_material(id).await? else {
        return Err(ApiError::not_found(
            "mfa.credential_not_found",
            "No such second factor",
        ));
    };
    // The step is not recorded here. Confirmation already costs a signed-in session, and this
    // is the one code the enrolling user has just read off their own screen — refusing it at
    // the login a moment later would make a successful enrolment look like a broken one.
    if matching_step(&state, &reference, &request.code)
        .await?
        .is_none()
    {
        return Err(ApiError::bad_request(
            "mfa.code_invalid",
            "That code did not match",
        ));
    }
    state.database.confirm_mfa_credential(id).await?;
    Ok(Json(MessageResponse::new(
        "mfa.enabled",
        "Two-factor sign-in is on",
    )))
}

#[utoipa::path(
    delete,
    path = "/api/v1/mfa/credentials/{id}",
    tag = "security",
    params(("id" = String, Path,)),
    responses((status = 200, body = MessageResponse))
)]
pub async fn delete_credential(
    State(state): State<AppState>,
    Path(id): Path<rd_core::MfaCredentialId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let Some(reference) = state.database.delete_mfa_credential(id).await? else {
        return Err(ApiError::not_found(
            "mfa.credential_not_found",
            "No such second factor",
        ));
    };
    // Best effort: an orphaned secret is inert, and failing the removal because of it would
    // leave the factor in place, which is the outcome the user asked against.
    if let Err(error) = state.secrets.remove(&reference).await {
        tracing::warn!(error = %error, "could not remove the second factor's stored secret");
    }
    Ok(Json(MessageResponse::new(
        "mfa.credential_removed",
        "Second factor removed",
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/mfa/disable",
    tag = "security",
    request_body = MfaDisableRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 401, description = "The password did not match", body = crate::error::ErrorBody),
    )
)]
pub async fn disable_mfa(
    State(state): State<AppState>,
    Json(request): Json<MfaDisableRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    // The password, not a code. Requiring the second factor to switch the second factor off
    // would make a lost phone unrecoverable, which is the failure this whole module is shaped
    // around avoiding.
    if !state
        .auth
        .password_matches(&state, &request.password)
        .await?
    {
        return Err(crate::error_codes::invalid_credentials());
    }
    // Scoped to the authenticator app. Somebody switching off the code prompt is not asking
    // to have their passkeys deleted, and silently taking a working way in is how a settings
    // toggle becomes a lockout.
    for reference in state.database.clear_mfa(rd_core::MfaKind::Totp).await? {
        if let Err(error) = state.secrets.remove(&reference).await {
            tracing::warn!(error = %error, "could not remove a second factor's stored secret");
        }
    }
    Ok(Json(MessageResponse::new(
        "mfa.disabled",
        "Two-factor sign-in is off",
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/mfa/recovery-codes",
    tag = "security",
    responses((status = 200, body = Vec<String>))
)]
pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
) -> Result<Json<Vec<String>>, ApiError> {
    let codes = rd_authn::recovery::generate_codes();
    state
        .database
        .replace_recovery_codes(codes.iter().map(|code| code.digest.clone()).collect())
        .await?;
    Ok(Json(codes.into_iter().map(|code| code.plaintext).collect()))
}

/// What a submitted second-factor code turned out to be, before anything was spent.
pub(crate) enum SecondFactorMatch {
    /// Nothing recognised it.
    None,
    /// An authenticator app's code, and the time step it belonged to.
    Totp {
        id: rd_core::MfaCredentialId,
        step: u64,
    },
    /// A recovery code, named by the digest that would be spent.
    Recovery { digest: String },
}

/// Which confirmed factor `code` answers. Spends nothing.
///
/// Deliberately split from [`consume_second_factor`], because the login has to do this work
/// whichever half of the credentials was wrong. Two things depend on the split:
///
/// * A recovery code must not be *spent* by somebody who does not also hold the password. It
///   used to be, so anyone who found a printout could burn the whole sheet permanently by
///   replaying it against a login that was refused anyway.
/// * The refusal must not become a password oracle. The secret-store read, the HMAC over the
///   accepted window and the digest comparison all happen here, before the password has
///   decided anything, so a wrong password does not return measurably sooner than a wrong code.
///
/// Returns `None` rather than an error for a code nothing recognises, so the caller decides
/// what to say — the login has to answer identically either way.
pub(crate) async fn second_factor_match(
    state: &AppState,
    code: &str,
) -> Result<SecondFactorMatch, ApiError> {
    for (id, reference) in state
        .database
        .confirmed_mfa_material(rd_core::MfaKind::Totp)
        .await?
    {
        if let Some(step) = matching_step(state, &reference, code).await? {
            return Ok(SecondFactorMatch::Totp { id, step });
        }
    }
    let stored = state.database.unused_recovery_digests().await?;
    if let Some(index) = rd_authn::recovery::find_match(code, &stored) {
        return Ok(SecondFactorMatch::Recovery {
            digest: stored[index].clone(),
        });
    }
    Ok(SecondFactorMatch::None)
}

/// Spends what [`second_factor_match`] found, and reports whether it was still spendable.
///
/// `false` for something that matched means it had already been used: a recovery code somebody
/// else redeemed, or a TOTP code being replayed inside its own drift window. Both have to look
/// exactly like a wrong code to the caller.
pub(crate) async fn consume_second_factor(
    state: &AppState,
    matched: SecondFactorMatch,
) -> Result<bool, ApiError> {
    match matched {
        SecondFactorMatch::None => Ok(false),
        SecondFactorMatch::Totp { id, step } => {
            // Recording the step is what makes the code one-time. TOTP accepts a window of one
            // step either side, so a code that is only noted as "used" stays valid for about
            // ninety seconds — long enough to replay one read over a shoulder, captured by a
            // phishing proxy or left in a client log. The writer refuses a step it has already
            // seen, in the same statement, so two requests arriving at once cannot both win.
            Ok(state
                .database
                .accept_totp_step(id, i64::try_from(step).unwrap_or(i64::MAX))
                .await?)
        }
        SecondFactorMatch::Recovery { digest } => {
            // Spending it is what makes a printout that leaks stop being a permanent bypass.
            // The `used_at IS NULL` in the statement also settles two requests arriving at once.
            Ok(state.database.spend_recovery_code(digest).await?)
        }
    }
}

/// The time step `code` answers for one stored secret, if any.
async fn matching_step(
    state: &AppState,
    reference: &str,
    code: &str,
) -> Result<Option<u64>, ApiError> {
    use secrecy::ExposeSecret;

    let stored = state.secrets.get(reference).await?;
    let Some(secret) = base32_decode(stored.expose_secret()) else {
        tracing::warn!("a stored second-factor secret is not readable");
        return Ok(None);
    };
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    Ok(rd_authn::totp::accepted_step(&secret, code, now))
}

/// The inverse of `rd_authn::totp::base32_encode`.
fn base32_decode(value: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0_u32;
    let mut accumulated = 0_u64;
    let mut out = Vec::new();
    for character in value.trim().bytes().filter(|byte| *byte != b'=') {
        let index = ALPHABET.iter().position(|entry| *entry == character)? as u64;
        accumulated = (accumulated << 5) | index;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((accumulated >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::base32_decode;

    /// The pair has to round-trip, or a secret written to the vault cannot be read back and
    /// the factor silently stops accepting every code.
    #[test]
    fn base32_round_trips_through_the_encoder() {
        for length in 0..40 {
            let bytes: Vec<u8> = (0..length).map(|index| (index * 7 + 3) as u8).collect();
            let encoded = rd_authn::totp::base32_encode(&bytes);
            assert_eq!(
                base32_decode(&encoded).as_deref(),
                Some(bytes.as_slice()),
                "length {length} did not survive the round trip"
            );
        }
    }

    #[test]
    fn a_secret_that_is_not_base32_is_refused_rather_than_guessed_at() {
        assert_eq!(base32_decode("not base32!"), None);
    }
}
