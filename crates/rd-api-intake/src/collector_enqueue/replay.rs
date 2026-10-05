//! The consent gate of a captured request.

use super::*;

/// The consent gate.
///
/// A captured request that would send something the browser sent — a POST, a body, or a
/// signed URL — may only be enqueued once a person has approved exactly this template. A
/// plain captured GET carries no credentials and needs no approval, so browser interception
/// behaves exactly as it did before replay existed.
pub(super) async fn replay_spec_for(
    state: &AppState,
    candidate: &LinkCandidate,
) -> Result<Option<rd_scheduler::ReplaySpec>, ApiError> {
    let Some(request) = candidate.request.clone() else {
        return Ok(None);
    };
    if !rd_core::needs_consent(&request) {
        return Ok(None);
    }
    if !request.replayable {
        let mut error = ApiError::conflict(
            crate::error_codes::REPLAY_NOT_REPLAYABLE,
            "This captured request cannot be reproduced",
        );
        if let Some(reason) = request.blocked_reason {
            // Serialized through serde so the parameter matches the `snake_case` variant
            // name the web UI translates.
            let reason = serde_json::to_string(&reason).unwrap_or_default();
            error = error.with_param("reason", reason.trim_matches('"'));
        }
        return Err(error);
    }
    let consent = state
        .database
        .candidate_replay_consent(candidate.id)
        .await?
        .ok_or_else(|| {
            ApiError::conflict(
                crate::error_codes::REPLAY_CONSENT_REQUIRED,
                "This download sends credentials and needs explicit approval first",
            )
            .with_param("candidate_id", candidate.id)
        })?;
    // Consent is bound to the template it was given for: a changed method, body or origin
    // set is a different decision, so it has to be made again.
    let expected = rd_core::stable_hash(&candidate.url, &request, None);
    if consent.template_hash != expected {
        return Err(ApiError::conflict(
            crate::error_codes::REPLAY_TEMPLATE_CHANGED,
            "The captured request changed since it was approved",
        )
        .with_param("candidate_id", candidate.id));
    }
    let body_ref = state.database.candidate_body_ref(candidate.id).await?;
    Ok(Some(rd_scheduler::ReplaySpec {
        request,
        consent,
        body_ref,
        candidate_id: Some(candidate.id),
    }))
}
