//! The authentication profile a LinkGrabber link is checked and queued with.

use super::*;

/// Sets the cookie/authentication profile a candidate is queued with (RD-080-04).
///
/// Validated here rather than in the store: a pinned profile that does not exist, is
/// disabled, or does not cover the link's host is a mistake worth reporting while the user
/// is still looking at the link, not one to discover when the download fails.
#[utoipa::path(
    put,
    path = "/api/v1/collector/candidates/{id}/auth-profile",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    request_body = crate::media_dto::CandidateAuthProfileRequest,
    responses(
        (status = 200, body = rd_core::LinkCandidate),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn put_candidate_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<crate::media_dto::CandidateAuthProfileRequest>,
) -> Result<Json<rd_core::LinkCandidate>, ApiError> {
    use crate::media_dto::CandidateAuthProfileMode;

    let candidate =
        state.database.get_candidate(id).await?.ok_or_else(|| {
            ApiError::not_found("collector.candidate_not_found", "Link not found")
        })?;

    let selection = match request.mode {
        CandidateAuthProfileMode::Auto => rd_core::AuthProfileSelection::Auto,
        CandidateAuthProfileMode::None => rd_core::AuthProfileSelection::None,
        CandidateAuthProfileMode::Pinned => {
            let profile_id = request.profile_id.ok_or_else(|| {
                ApiError::bad_request(
                    "collector.auth_profile_missing",
                    "Pinning a profile requires a profile id",
                )
            })?;
            let profile = state
                .database
                .auth_profile(profile_id)
                .await?
                .ok_or_else(|| {
                    ApiError::not_found(
                        "collector.auth_profile_not_found",
                        "Authentication profile not found",
                    )
                })?;
            if !profile.enabled {
                return Err(ApiError::unprocessable(
                    "collector.auth_profile_disabled",
                    "That authentication profile is disabled",
                ));
            }
            // Refused up front: a profile scoped to another site would be materialised into
            // an empty cookie file and fail at download time, which is a worse place to
            // learn about it.
            if !profile.scope.matches_url(&candidate.url) {
                return Err(ApiError::unprocessable(
                    "collector.auth_profile_scope_mismatch",
                    "That profile does not cover this link's address",
                )
                .with_param("host", profile.scope.host.clone()));
            }
            rd_core::AuthProfileSelection::Pinned(profile_id)
        }
    };
    let candidate = state
        .database
        .set_candidate_auth_profile(id, selection)
        .await
        .map_err(|error| ApiError::conflict("collector.candidate_busy", error.to_string()))?;
    Ok(Json(candidate))
}
