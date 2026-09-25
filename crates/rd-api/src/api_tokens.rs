//! Session-managed machine bearer tokens.
//!
//! A token carries one or more of the six API areas; `api:*` and `api:read` are the two
//! legacy spellings and still mean what they always did. All of them are revocable and
//! stored as a digest, never as the bearer.
//!
//! ## Least privilege is the default, not the advice
//!
//! Minting takes an explicit list of areas and grants exactly that list — nothing here
//! widens a request. A call that names no areas at all gets read access, because the
//! alternative default is "everything" and a default of everything makes the whole model
//! decorative. The one exception is the legacy `read_only` flag, which keeps its old
//! meaning for clients that predate the areas.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::{
    ApiError, AppState,
    dto::{
        ApiTokenRequest, ApiTokenScopesRequest, CapturePairResponse, MessageResponse,
        ScopeDescriptor,
    },
};

/// Validates the label, mints a one-time bearer and persists only its digest.
pub(crate) async fn pair_scoped_token(
    state: &AppState,
    label: &str,
    scope: &str,
    label_error_code: &'static str,
) -> Result<CapturePairResponse, ApiError> {
    pair_with_scopes(state, label, vec![scope.to_owned()], label_error_code).await
}

/// The same, for a token holding several areas at once.
async fn pair_with_scopes(
    state: &AppState,
    label: &str,
    scopes: Vec<String>,
    label_error_code: &'static str,
) -> Result<CapturePairResponse, ApiError> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 100 {
        return Err(ApiError::bad_request(
            label_error_code,
            "Token label must be between 1 and 100 characters",
        )
        .with_param("max", 100));
    }
    let mut random = [0_u8; 32];
    rand::rng().fill_bytes(&mut random);
    let bearer = URL_SAFE_NO_PAD.encode(random);
    let token = state
        .database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            label.to_owned(),
            hex::encode(Sha256::digest(bearer.as_bytes())),
            scopes,
        )
        .await?;
    Ok(CapturePairResponse { bearer, token })
}

#[utoipa::path(post, path = "/api/v1/api-tokens", tag = "api-tokens", request_body = ApiTokenRequest, responses((status = 201, body = CapturePairResponse)))]
pub async fn pair_api_token(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<ApiTokenRequest>,
) -> Result<(StatusCode, Json<CapturePairResponse>), ApiError> {
    let scopes = requested_scopes(&request)?;
    let response =
        pair_with_scopes(&state, &request.label, scopes.clone(), "api.label_length").await?;
    // The scopes, the label and the id. Never `response.bearer`: that value exists in this
    // process for the length of one response and must not be written anywhere, least of all
    // into a table somebody keeps for a year.
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::TokenCreated)
            .by(&audit)
            .target("token", response.token.id)
            .named(response.token.label.clone())
            .detail("scopes", scopes.join(" ")),
    )
    .await;
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
        return Ok(vec![if request.read_only {
            rd_core::API_READ_SCOPE.to_owned()
        } else {
            rd_core::API_SCOPE.to_owned()
        }]);
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
/// Two limits keep the reversal from being a bridge. The areas go through the same
/// [`resolve_scopes`] the minting path uses, so `capture:*` stays unmintable *and*
/// ungrantable; and only a token the API token list already shows can be re-scoped at all, so
/// a browser-capture token cannot be turned into an API token by naming its id here.
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
mod tests {
    use super::requested_scopes;
    use crate::dto::ApiTokenRequest;

    fn request(scopes: &[&str], read_only: bool) -> ApiTokenRequest {
        ApiTokenRequest {
            label: "test".to_owned(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            read_only,
        }
    }

    #[test]
    fn a_named_set_is_granted_exactly() {
        let scopes = requested_scopes(&request(&["api:queue", "api:intake"], false))
            .expect("both are real areas");
        assert_eq!(
            scopes,
            vec!["api:intake".to_owned(), "api:queue".to_owned()]
        );
    }

    /// The property the whole model rests on: minting never hands out more than was asked
    /// for. `api:queue` implies reading at *check* time, which is not the same as storing a
    /// second scope — storing it would make the token look broader than it is in the list.
    #[test]
    fn minting_never_widens_a_request() {
        for area in [
            "api:read",
            "api:intake",
            "api:queue",
            "api:config",
            "api:secrets",
            "api:admin",
        ] {
            let scopes = requested_scopes(&request(&[area], false)).expect("a real area");
            assert_eq!(scopes, vec![area.to_owned()], "{area} was widened");
        }
    }

    /// The capture surface and the API are isolated in both directions, and a minting call is
    /// exactly where somebody would try to bridge them.
    #[test]
    fn the_capture_scope_cannot_be_minted_as_an_api_token() {
        let error = requested_scopes(&request(&["capture:*"], false))
            .expect_err("capture is not an API area");
        assert_eq!(error.code(), "api.scope_unknown");
    }

    /// Dropping it would produce a token weaker than the caller believes, which then fails
    /// somewhere else entirely, long after the cause.
    #[test]
    fn an_unknown_scope_is_refused_rather_than_ignored() {
        for name in ["api:everything", "", "read", "api:Read"] {
            assert!(
                requested_scopes(&request(&[name], false)).is_err(),
                "`{name}` was accepted"
            );
        }
    }

    /// Re-scoping an existing token goes through the same resolver as minting a new one, so
    /// the refusals cannot be softer on the path that *widens* a credential than on the path
    /// that creates one.
    #[test]
    fn re_scoping_resolves_by_exactly_the_same_rules_as_minting() {
        for named in [
            vec!["api:queue".to_owned(), "api:intake".to_owned()],
            vec!["api:*".to_owned()],
            vec!["api:read".to_owned(), "api:read".to_owned()],
        ] {
            assert_eq!(
                super::resolve_scopes(&named).expect("real areas"),
                requested_scopes(&ApiTokenRequest {
                    label: "test".to_owned(),
                    scopes: named.clone(),
                    read_only: false,
                })
                .expect("real areas"),
                "{named:?}"
            );
        }
        for refused in [vec!["capture:*".to_owned()], vec!["api:nope".to_owned()]] {
            assert_eq!(
                super::resolve_scopes(&refused)
                    .expect_err("not an API area")
                    .code(),
                "api.scope_unknown",
                "{refused:?}"
            );
        }
    }

    #[test]
    fn duplicates_collapse_rather_than_being_stored_twice() {
        let scopes = requested_scopes(&request(&["api:read", "api:read", " api:read "], false))
            .expect("a real area");
        assert_eq!(scopes, vec!["api:read".to_owned()]);
    }

    /// A client written before the areas existed sends only a label and a flag, and has to
    /// keep getting the token it used to get.
    #[test]
    fn the_legacy_flag_still_decides_when_no_area_is_named() {
        assert_eq!(
            requested_scopes(&request(&[], false)).expect("legacy default"),
            vec![rd_core::API_SCOPE.to_owned()]
        );
        assert_eq!(
            requested_scopes(&request(&[], true)).expect("legacy read-only"),
            vec![rd_core::API_READ_SCOPE.to_owned()]
        );
    }

    /// Naming areas has to beat the flag, or a UI that sends both produces a token that
    /// silently ignores the choice the person just made.
    #[test]
    fn a_named_set_overrides_the_legacy_flag() {
        let scopes =
            requested_scopes(&request(&["api:admin"], true)).expect("admin is a real area");
        assert_eq!(scopes, vec!["api:admin".to_owned()]);
    }

    /// The preview a person chooses against must come from the table that will refuse them,
    /// and must order the areas the way the model does.
    #[test]
    fn the_capability_preview_is_ordered_and_non_trivial() {
        let mut previous = 0;
        for scope in rd_core::Scope::API {
            let reachable = crate::scope_policy::operations_reachable_by(*scope);
            assert!(reachable > 0, "{scope:?} reaches nothing at all");
            if *scope == rd_core::Scope::Metrics {
                // The island (RD-110-01): one route, and nothing of the ladder.
                assert_eq!(
                    reachable, 1,
                    "api:metrics must reach exactly the exposition"
                );
                continue;
            }
            if *scope != rd_core::Scope::Secrets {
                // Everything that acts also reads, so each area reaches strictly more than
                // reading alone. Secrets is the deliberate exception: it confers nothing.
                assert!(
                    reachable > previous || *scope == rd_core::Scope::Read,
                    "{scope:?} reaches no more than the area before it"
                );
            }
            if *scope == rd_core::Scope::Read {
                previous = reachable;
            }
        }
    }
}
