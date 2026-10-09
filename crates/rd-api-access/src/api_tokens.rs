//! Session-managed machine bearer tokens.
//!
//! A token carries one or more of the API areas, or `api:*` for all of them at once. All of
//! them are revocable and stored as a digest, never as the bearer.
//!
//! ## Least privilege is the default, not the advice
//!
//! Minting takes an explicit list of areas and grants exactly that list — nothing here
//! widens a request. A call that names no areas at all gets read access, because the
//! alternative default is "everything" and a default of everything makes the whole model
//! decorative.
//!
//! ## Nobody hands out more than they hold
//!
//! Minting and re-scoping cost `api:secrets`, and `api:secrets` confers nothing else — so
//! without a ceiling, a token holding only that area could mint `api:*` or re-scope itself to
//! it, and the ladder would end at the credentials area (security audit 2026-09-30, finding 1).
//! Every area a request names, with everything it implies, has to be one the caller's own
//! credential holds. A session holds every area, so the administrator is not limited by this.

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use rd_api_core::input_checks::{TextLimit, required_text};

use crate::{
    ApiError, AppState,
    auth::Granted,
    dto::{
        ApiTokenLimitsRequest, ApiTokenRequest, ApiTokenScopesRequest, CapturePairResponse,
        MessageResponse, ScopeDescriptor,
    },
};

/// The longest expiry a token can be given, in days: ten years, which is "never" for anybody
/// who would rather not say so.
const MAX_EXPIRY_DAYS: u32 = 3650;

/// The highest call limit per minute a token can be given (RD-1200-04): a hundred a second,
/// beyond which a limit no longer limits anything a person would notice.
const MAX_CALLS_PER_MINUTE: u32 = 6000;

/// Validates the label, mints a one-time bearer holding `scopes` and persists only its digest.
///
/// `expires_in_days` is the optional expiry (RD-1110-07); `None` never expires.
/// `calls_per_minute` is the optional call limit (RD-1200-04); `None` is no limit.
pub(crate) async fn pair_with_scopes(
    state: &AppState,
    label: &str,
    scopes: Vec<String>,
    expires_in_days: Option<u32>,
    calls_per_minute: Option<u32>,
    label_error_code: &'static str,
) -> Result<CapturePairResponse, ApiError> {
    let label = required_text(
        label,
        TextLimit::Chars(100),
        label_error_code,
        "Token label must be between 1 and 100 characters",
    )?;
    let expires_at = expiry(expires_in_days)?;
    let calls_per_minute = call_limit(calls_per_minute)?;
    let mut random = [0_u8; 32];
    rand::rng().fill_bytes(&mut random);
    let bearer = URL_SAFE_NO_PAD.encode(random);
    let token = state
        .database
        .create_limited_capture_token(
            rd_core::CaptureTokenId::new(),
            label,
            rd_authn::sha256_hex(&bearer),
            scopes,
            expires_at,
            calls_per_minute,
        )
        .await?;
    Ok(CapturePairResponse { bearer, token })
}

/// The moment a token asked to expire after `days` stops being accepted, or `None` for one
/// that never expires. Out of range is refused, not clamped: a token that silently lives
/// shorter or longer than its caller asked is the surprise this check exists to prevent.
fn expiry(days: Option<u32>) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    let Some(days) = days else {
        return Ok(None);
    };
    if !(1..=MAX_EXPIRY_DAYS).contains(&days) {
        return Err(ApiError::bad_request(
            "api.token_expiry_range",
            "A token expires after 1 to 3650 days, or never",
        )
        .with_param("max", MAX_EXPIRY_DAYS));
    }
    Ok(Some(
        chrono::Utc::now() + chrono::Duration::days(i64::from(days)),
    ))
}

/// A call limit inside the range, or none. Out of range is refused, not clamped, like the
/// expiry: a limit other than the one asked for is a surprise either way.
fn call_limit(calls_per_minute: Option<u32>) -> Result<Option<u32>, ApiError> {
    match calls_per_minute {
        Some(limit) if !(1..=MAX_CALLS_PER_MINUTE).contains(&limit) => Err(ApiError::bad_request(
            "api.token_rate_range",
            "A token makes 1 to 6000 calls per minute, or has no limit",
        )
        .with_param("max", MAX_CALLS_PER_MINUTE)),
        limit => Ok(limit),
    }
}

#[utoipa::path(post, path = "/api/v1/api-tokens", tag = "api-tokens", request_body = ApiTokenRequest, responses((status = 201, body = CapturePairResponse)))]
pub async fn pair_api_token(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    granted: Option<Extension<Granted>>,
    Json(request): Json<ApiTokenRequest>,
) -> Result<(StatusCode, Json<CapturePairResponse>), ApiError> {
    let scopes = requested_scopes(&request)?;
    within_grant(granted.as_ref().map(|Extension(granted)| granted), &scopes)?;
    let response = pair_with_scopes(
        &state,
        &request.label,
        scopes.clone(),
        request.expires_in_days,
        request.calls_per_minute,
        "api.label_length",
    )
    .await?;
    // The scopes, the label, the id and the expiry. Never `response.bearer`: that value exists
    // in this process for the length of one response and must not be written anywhere, least
    // of all into a table somebody keeps for a year.
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::TokenCreated)
        .by(&audit)
        .target("token", response.token.id)
        .named(response.token.label.clone())
        .detail("scopes", scopes.join(" "));
    if let Some(expires_at) = response.token.expires_at {
        event = event.detail("expires_at", expires_at.to_rfc3339());
    }
    if let Some(limit) = response.token.calls_per_minute {
        event = event.detail("calls_per_minute", limit);
    }
    crate::audit::record(&state, event).await;
    Ok((StatusCode::CREATED, Json(response)))
}

/// Turns a minting request into the exact scope strings to store.
///
/// Every named scope must be a real API area. Two refusals matter more than the rest:
/// `capture:*` is rejected outright, because the capture surface and the API are isolated in
/// both directions and a minting call is exactly where somebody would try to bridge them; and
/// an unknown string is refused rather than dropped, since silently ignoring a scope produces
/// a token that is weaker than the caller believes and fails much later, somewhere else.
fn requested_scopes(request: &ApiTokenRequest) -> Result<Vec<String>, ApiError> {
    if request.scopes.is_empty() {
        return Ok(vec![rd_core::API_READ_SCOPE.to_owned()]);
    }
    resolve_scopes(&request.scopes)
}

/// The shared half of [`requested_scopes`]: every named area, resolved or refused.
///
/// Split out so re-scoping an existing token cannot grow a second, more forgiving idea of
/// what a scope string means. The refusals below are the reason: they are worth nothing if a
/// later call gets to bypass them.
fn resolve_scopes(requested_names: &[String]) -> Result<Vec<String>, ApiError> {
    let mut resolved = Vec::new();
    for requested in requested_names {
        let name = requested.trim();
        // `api:*` stays mintable: it is what "everything" is spelled as, and refusing it here
        // would only push callers to list all six by hand for the same result.
        if name == rd_core::API_SCOPE {
            resolved.push(rd_core::API_SCOPE.to_owned());
            continue;
        }
        let scope = rd_core::Scope::parse(name).filter(|scope| rd_core::Scope::API.contains(scope));
        let Some(scope) = scope else {
            return Err(ApiError::bad_request(
                "api.scope_unknown",
                "That is not an API permission",
            )
            .with_param("scope", name));
        };
        resolved.push(scope.as_str().to_owned());
    }
    resolved.sort_unstable();
    resolved.dedup();
    Ok(resolved)
}

/// Refuses a set of areas the caller's own credential does not cover.
///
/// Compared after expansion on both sides: `api:queue` implies reading, so handing it out needs
/// reading too, and `api:*` needs every area there is. A missing grant — a handler mounted
/// outside the session layer — covers nothing, the same safe reading the event stream gives it.
fn within_grant(granted: Option<&Granted>, scopes: &[String]) -> Result<(), ApiError> {
    let exceeded = rd_core::granted_scopes(scopes.iter().map(String::as_str))
        .into_iter()
        .find(|scope| !granted.is_some_and(|granted| granted.holds(*scope)));
    match exceeded {
        None => Ok(()),
        Some(scope) => Err(ApiError::forbidden(
            "api.scope_exceeds_grant",
            "A token cannot be given an area the credential handing it out does not hold",
        )
        .with_param("scope", scope.as_str())),
    }
}

/// The areas a token can be given, with what each one actually reaches.
#[utoipa::path(
    get,
    path = "/api/v1/api-tokens/scopes",
    tag = "api-tokens",
    responses((status = 200, body = [ScopeDescriptor]))
)]
pub async fn list_token_scopes() -> Json<Vec<ScopeDescriptor>> {
    Json(
        rd_core::Scope::API
            .iter()
            .map(|scope| ScopeDescriptor {
                scope: scope.as_str().to_owned(),
                implies: scope
                    .implies()
                    .iter()
                    .map(|implied| implied.as_str().to_owned())
                    .collect(),
                operations: crate::scope_policy::operations_reachable_by(*scope),
                sensitive: matches!(scope, rd_core::Scope::Secrets | rd_core::Scope::Admin),
            })
            .collect(),
    )
}

#[utoipa::path(get, path = "/api/v1/api-tokens", tag = "api-tokens", responses((status = 200, body = [rd_core::CaptureToken])))]
pub async fn list_api_tokens(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::CaptureToken>>, ApiError> {
    // Every API area, plus the two legacy spellings. Filtering on the old pair would hide a
    // token minted for, say, the queue alone — which is precisely the kind of token the areas
    // exist to make possible, and a token you cannot see is a token you cannot revoke.
    Ok(Json(
        state
            .database
            .list_capture_tokens(&api_token_scopes())
            .await?,
    ))
}

/// The scope strings that make up the API token surface, as the list route filters on them.
///
/// One definition rather than two: re-scoping decides what an API token *is* by the same
/// membership the list uses, so a token the list refuses to show is a token this route
/// refuses to touch.
fn api_token_scopes() -> Vec<&'static str> {
    let mut scopes = rd_core::Scope::API
        .iter()
        .map(|scope| scope.as_str())
        .collect::<Vec<_>>();
    scopes.push(rd_core::API_SCOPE);
    scopes
}

/// Replaces the areas of an existing token, keeping its bearer value.
///
/// ## Why this exists, given that it did not
///
/// A token used to be born with its areas and keep them until it was revoked, and that was a
/// real promise: a leaked bearer could never become more dangerous than it was on the day it
/// leaked. Re-scoping gives that up, and the trade is deliberate. The alternative in practice
/// was not "narrow tokens forever" — it was a person minting `api:*` up front because
/// reconnecting every client to widen a token later is expensive, which is a worse outcome for
/// exactly the same leak. What replaces the old guarantee is the record: issuing, re-scoping
/// and revoking all write an event, so "when did this token gain that area" has an answer.
///
/// Three limits keep the reversal from being a bridge. The areas go through the same
/// [`resolve_scopes`] the minting path uses, so `capture:*` stays unmintable *and*
/// ungrantable; only a token the API token list already shows can be re-scoped at all, so
/// a browser-capture token cannot be turned into an API token by naming its id here; and the
/// new areas must lie within the caller's own ([`within_grant`]).
#[utoipa::path(
    patch,
    path = "/api/v1/api-tokens/{id}",
    tag = "api-tokens",
    request_body = ApiTokenScopesRequest,
    params(("id" = rd_core::CaptureTokenId, Path)),
    responses((status = 200, body = rd_core::CaptureToken), (status = 400), (status = 404))
)]
pub async fn update_api_token_scopes(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    granted: Option<Extension<Granted>>,
    Path(id): Path<rd_core::CaptureTokenId>,
    Json(request): Json<ApiTokenScopesRequest>,
) -> Result<Json<rd_core::CaptureToken>, ApiError> {
    // No legacy fallback here, unlike minting: an empty list has no old meaning to honour, and
    // reading it as "everything" or as "nothing" would both be guesses. Removing all access is
    // spelled by revoking.
    if request.scopes.is_empty() {
        return Err(ApiError::bad_request(
            "api.scopes_empty",
            "Name at least one API area, or revoke the token instead",
        ));
    }
    let scopes = resolve_scopes(&request.scopes)?;
    // The same ceiling as minting, and the reason a token cannot re-scope itself upwards: its
    // own grant is the one compared against.
    within_grant(granted.as_ref().map(|Extension(granted)| granted), &scopes)?;
    let tokens = state
        .database
        .list_capture_tokens(&api_token_scopes())
        .await?;
    if !tokens.iter().any(|token| token.id == id) {
        return Err(ApiError::not_found(
            "api.token_not_found",
            "API token not found",
        ));
    }
    let previous = tokens
        .iter()
        .find(|token| token.id == id)
        .map(|token| token.scopes.join(" "))
        .unwrap_or_default();
    let token = state
        .database
        .update_capture_token_scopes(id, scopes)
        .await
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "api.token_not_found",
                "API token not found",
            )
        })?;
    // The one route that can *widen* a credential, so both sides of the change are recorded.
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::TokenRescoped)
            .by(&audit)
            .target("token", id)
            .named(token.label.clone())
            .detail("scopes_before", previous)
            .detail("scopes_after", token.scopes.join(" ")),
    )
    .await;
    Ok(Json(token))
}

/// Sets or clears an existing token's call limit per minute (RD-1200-04), keeping its bearer
/// value and its areas.
///
/// Only a token the API token list shows, as for re-scoping, and recorded with both sides of
/// the change: lifting a limit is the kind of change the audit log is asked about afterwards.
#[utoipa::path(
    put,
    path = "/api/v1/api-tokens/{id}/limits",
    tag = "api-tokens",
    request_body = ApiTokenLimitsRequest,
    params(("id" = rd_core::CaptureTokenId, Path)),
    responses((status = 200, body = rd_core::CaptureToken), (status = 400), (status = 404))
)]
pub async fn update_api_token_limits(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(id): Path<rd_core::CaptureTokenId>,
    Json(request): Json<ApiTokenLimitsRequest>,
) -> Result<Json<rd_core::CaptureToken>, ApiError> {
    let calls_per_minute = call_limit(request.calls_per_minute)?;
    let tokens = state
        .database
        .list_capture_tokens(&api_token_scopes())
        .await?;
    let Some(previous) = tokens.iter().find(|token| token.id == id) else {
        return Err(ApiError::not_found(
            "api.token_not_found",
            "API token not found",
        ));
    };
    let before = previous.calls_per_minute;
    let token = state
        .database
        .update_capture_token_limits(id, calls_per_minute)
        .await
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "api.token_not_found",
                "API token not found",
            )
        })?;
    let spelled =
        |limit: Option<u32>| limit.map_or_else(|| "none".to_owned(), |limit| limit.to_string());
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::TokenLimitsChanged)
            .by(&audit)
            .target("token", id)
            .named(token.label.clone())
            .detail("calls_per_minute_before", spelled(before))
            .detail("calls_per_minute_after", spelled(token.calls_per_minute)),
    )
    .await;
    Ok(Json(token))
}

#[utoipa::path(delete, path = "/api/v1/api-tokens/{id}", tag = "api-tokens", params(("id" = rd_core::CaptureTokenId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn revoke_api_token(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(id): Path<rd_core::CaptureTokenId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let label = state
        .database
        .list_capture_tokens(&api_token_scopes())
        .await
        .ok()
        .and_then(|tokens| {
            tokens
                .into_iter()
                .find(|token| token.id == id)
                .map(|token| token.label)
        });
    state
        .database
        .revoke_capture_token(id)
        .await
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "api.token_not_found",
                "API token not found",
            )
        })?;
    let mut event = crate::audit::AuditEvent::success(rd_core::AuditAction::TokenRevoked)
        .by(&audit)
        .target("token", id);
    if let Some(label) = label {
        event = event.named(label);
    }
    crate::audit::record(&state, event).await;
    Ok(Json(MessageResponse::new(
        "api.token_revoked",
        "API token revoked",
    )))
}

#[cfg(test)]
#[path = "api_tokens_tests.rs"]
mod tests;
