//! Preview and consent for replaying an authenticated captured request.
//!
//! The gate itself lives in `collector_enqueue`; these endpoints are what lets a person see
//! what would be sent and decide. They are deliberately API-first: the web UI's modal is a
//! presentation of this contract, and nothing here depends on the UI existing.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::{CandidateId, LinkCandidate, ReplayConsent, ReplayMethod};

use crate::{
    ApiError, AppState,
    dto::MessageResponse,
    error_codes::{
        REPLAY_CONSENT_REQUIRED, REPLAY_NOT_REPLAYABLE, REPLAY_ORIGIN_NOT_APPROVED,
        REPLAY_TEMPLATE_CHANGED, REPLAY_TEMPLATE_MISSING, parse_id,
    },
    replay_dto::{AuthProfileSummary, ReplayConsentRequest, ReplayPreviewResponse},
};

/// Loads a candidate that actually carries a captured request.
async fn candidate_with_request(
    state: &AppState,
    id: CandidateId,
) -> Result<LinkCandidate, ApiError> {
    let candidate = state.database.get_candidate(id).await?.ok_or_else(|| {
        ApiError::not_found("collector.candidate_not_found", "Candidate not found")
    })?;
    if candidate.request.is_none() {
        return Err(ApiError::conflict(
            REPLAY_TEMPLATE_MISSING,
            "This link carries no captured request",
        ));
    }
    Ok(candidate)
}

/// Builds the preview a person approves against.
async fn preview_for(
    state: &AppState,
    candidate: &LinkCandidate,
) -> Result<ReplayPreviewResponse, ApiError> {
    let request = candidate.request.clone().unwrap_or_default();
    // The profile is matched the same way the transfer will match it, so the preview cannot
    // promise one thing and the download send another.
    let profile = state.database.match_auth_profile(&candidate.url).await?;
    let auth_profile = profile.as_ref().map(|profile| AuthProfileSummary {
        id: profile.id,
        name: profile.name.clone(),
        method: profile.method,
        scope_host: profile.scope.host.clone(),
        has_client_certificate: profile.has_client_certificate,
        expires_at: profile.expires_at,
    });
    let target_origin =
        rd_core::origin_of(request.effective_url.as_ref().unwrap_or(&candidate.url))
            .unwrap_or_default();
    Ok(ReplayPreviewResponse {
        candidate_id: candidate.id,
        url: rd_core::redact_url(&candidate.url),
        effective_url: request.effective_url.as_ref().map(rd_core::redact_url),
        target_origin,
        approved_origins: request.approved_origins.clone(),
        method: ReplayMethod::parse(&request.method).unwrap_or_default(),
        content_type: request.body.as_ref().map(|body| body.content_type.clone()),
        body: request.body.clone(),
        headers: request.headers.clone(),
        credential_categories: rd_core::credential_categories(
            &candidate.url,
            &request,
            profile.as_ref(),
        ),
        auth_profile,
        expires_at: request.expires_at,
        replayable: request.replayable,
        blocked_reason: request.blocked_reason,
        template_hash: rd_core::stable_hash(&candidate.url, &request, None),
        consent: state
            .database
            .candidate_replay_consent(candidate.id)
            .await?,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/collector/candidates/{id}/replay-preview",
    tag = "collector",
    params(("id" = String, Path, description = "Candidate id")),
    responses(
        (status = 200, body = ReplayPreviewResponse),
        (status = 404, description = "Candidate not found"),
        (status = 409, description = "The candidate carries no captured request")
    )
)]
pub async fn replay_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ReplayPreviewResponse>, ApiError> {
    let id = parse_id::<CandidateId>(&id)?;
    let candidate = candidate_with_request(&state, id).await?;
    Ok(Json(preview_for(&state, &candidate).await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/collector/candidates/{id}/replay-consent",
    tag = "collector",
    params(("id" = String, Path, description = "Candidate id")),
    request_body = ReplayConsentRequest,
    responses(
        (status = 200, body = ReplayPreviewResponse),
        (status = 400, description = "An origin outside the derived set was named"),
        (status = 404, description = "Candidate not found"),
        (status = 409, description = "The capture changed, or cannot be reproduced")
    )
)]
pub async fn grant_replay_consent(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReplayConsentRequest>,
) -> Result<Json<ReplayPreviewResponse>, ApiError> {
    let id = parse_id::<CandidateId>(&id)?;
    let candidate = candidate_with_request(&state, id).await?;
    let request = candidate.request.clone().unwrap_or_default();

    if !request.replayable {
        let mut error = ApiError::conflict(
            REPLAY_NOT_REPLAYABLE,
            "This captured request cannot be reproduced",
        );
        if let Some(reason) = request.blocked_reason {
            let reason = serde_json::to_string(&reason).unwrap_or_default();
            error = error.with_param("reason", reason.trim_matches('"'));
        }
        return Err(error);
    }

    // Approval is bound to the exact template that was shown. Anything else would let a
    // capture change between the moment a person read the dialog and the moment they agreed.
    let expected = rd_core::stable_hash(&candidate.url, &request, None);
    if body.template_hash != expected {
        return Err(ApiError::conflict(
            REPLAY_TEMPLATE_CHANGED,
            "The captured request changed since this preview was shown",
        )
        .with_param("candidate_id", id));
    }

    // A client may narrow the origin set but never widen it: the server derived it from the
    // capture, and a wider set would send credentials somewhere the browser never did.
    let approved_origins = if body.approved_origins.is_empty() {
        request.approved_origins.clone()
    } else {
        for origin in &body.approved_origins {
            if !request.approved_origins.contains(origin) {
                return Err(ApiError::bad_request(
                    REPLAY_ORIGIN_NOT_APPROVED,
                    "That origin is not part of the captured request",
                )
                .with_param("origin", origin));
            }
        }
        body.approved_origins.clone()
    };

    state
        .database
        .set_candidate_replay_consent(
            id,
            Some(ReplayConsent {
                granted_at: chrono::Utc::now(),
                template_hash: expected,
                approved_origins,
            }),
        )
        .await?;
    Ok(Json(preview_for(&state, &candidate).await?))
}

#[utoipa::path(
    delete,
    path = "/api/v1/collector/candidates/{id}/replay-consent",
    tag = "collector",
    params(("id" = String, Path, description = "Candidate id")),
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, description = "Candidate not found")
    )
)]
pub async fn revoke_replay_consent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    let id = parse_id::<CandidateId>(&id)?;
    let candidate = candidate_with_request(&state, id).await?;
    state
        .database
        .set_candidate_replay_consent(candidate.id, None)
        .await?;
    Ok(Json(MessageResponse::new(
        "replay.consent_revoked",
        "Replay approval withdrawn",
    )))
}

/// Keeps the enqueue gate's code reachable from this module's documentation.
///
/// The gate returns [`REPLAY_CONSENT_REQUIRED`] when a capture that would send credentials
/// is enqueued without approval; these endpoints are how a client resolves that.
const _: &str = REPLAY_CONSENT_REQUIRED;
