//! The links inside the LinkGrabber packages: moving, ordering, checking and renaming them.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_api_core::list_bounds::validate_bulk;
use rd_core::{CandidateId, CollectorPackage, LinkCandidateState};
use rd_db::{MoveTarget, StoreErrorKind};

use super::message;
use crate::{
    ApiError, AppState,
    dto::{
        CandidateCheckRequest, CandidateMoveRequest, CandidateRenameRequest,
        CandidateReorderRequest, MessageResponse,
    },
};

#[utoipa::path(post, path = "/api/v1/collector/candidates/move", tag = "collector", request_body = CandidateMoveRequest, responses((status = 200, body = rd_core::CollectorPackage)))]
pub async fn move_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateMoveRequest>,
) -> Result<Json<CollectorPackage>, ApiError> {
    validate_bulk(request.ids.len())?;
    let target = match (request.package_id, request.new_package_name) {
        (Some(id), _) => MoveTarget::Existing(id),
        (None, Some(name)) if !name.trim().is_empty() => MoveTarget::New { name },
        _ => {
            return Err(ApiError::bad_request(
                "collector.move_target_missing",
                "Target package or new package name is missing",
            ));
        }
    };
    Ok(Json(
        state.database.move_candidates(request.ids, target).await?,
    ))
}

/// Writes the manual link order inside one package.
///
/// The list has to be exactly the package's links, each of them once: the store hands out the
/// positions 1..n from it, and its `UPDATE` is fenced by `package_id`, so a foreign id used to
/// be a silent no-op that still answered "Order saved". `/api/v1/downloads/reorder` applies the
/// same rule to the download queue.
#[utoipa::path(post, path = "/api/v1/collector/candidates/reorder", tag = "collector", request_body = CandidateReorderRequest, responses((status = 200, body = MessageResponse), (status = 400)))]
pub async fn reorder_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateReorderRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    crate::error_codes::validate_reorder_size(request.ids.len())?;
    let members: Vec<rd_core::CandidateId> = state
        .database
        .list_candidates()
        .await?
        .into_iter()
        .filter(|candidate| candidate.package_id == Some(request.package_id))
        .map(|candidate| candidate.id)
        .collect();
    crate::error_codes::validate_reorder(&members, &request.ids)?;
    state
        .database
        .reorder_candidates(request.package_id, request.ids)
        .await?;
    Ok(message("collector.order_saved", "Order saved"))
}

#[utoipa::path(post, path = "/api/v1/collector/candidates/check", tag = "collector", request_body = CandidateCheckRequest, responses((status = 202, body = MessageResponse)))]
pub async fn check_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateCheckRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    let ids = match request.ids {
        Some(ids) if !ids.is_empty() => ids,
        _ => state
            .database
            .list_candidates()
            .await?
            .into_iter()
            .filter(|candidate| {
                !matches!(
                    candidate.state,
                    LinkCandidateState::Checking | LinkCandidateState::Resolving
                )
            })
            .map(|candidate| candidate.id)
            .collect(),
    };
    let count = ids.len();
    state.link_check.check(ids).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            MessageResponse::new(
                "collector.check_started",
                format!("Check started for {count} link(s)"),
            )
            .with_count(count),
        ),
    ))
}

#[utoipa::path(patch, path = "/api/v1/collector/candidates/{id}", tag = "collector", params(("id" = rd_core::CandidateId, Path)), request_body = CandidateRenameRequest, responses((status = 200, body = rd_core::LinkCandidate), (status = 404)))]
pub async fn rename_candidate(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<CandidateRenameRequest>,
) -> Result<Json<rd_core::LinkCandidate>, ApiError> {
    if let Some(variant) = request.media_variant.as_deref().map(str::trim) {
        let candidate = state
            .database
            .set_candidate_media_variant(id, variant.to_owned())
            .await
            .map_err(|error| match rd_db::store_kind(&error) {
                Some(StoreErrorKind::NotFound) => {
                    ApiError::not_found("collector.candidate_not_found", "Link not found")
                }
                Some(StoreErrorKind::UnknownMediaVariant) => {
                    ApiError::bad_request("media.variant_unknown", "Unknown media variant")
                }
                Some(StoreErrorKind::NoMediaMetadata) => {
                    ApiError::bad_request("media.not_media_link", "This link has no media variants")
                }
                _ => ApiError::conflict("collector.candidate_busy", "Link is busy"),
            })?;
        if request.file_name.is_none() {
            return Ok(Json(candidate));
        }
    }
    let Some(file_name) = request.file_name.as_deref() else {
        return Err(ApiError::bad_request(
            "package.no_change",
            "No change specified",
        ));
    };
    let file_name = rd_files::sanitize_file_name(file_name.trim());
    if file_name.is_empty() || file_name.chars().count() > 255 {
        return Err(ApiError::bad_request(
            "collector.file_name_length",
            "File name must be between 1 and 255 characters",
        )
        .with_param("max", 255));
    }
    state
        .database
        .set_candidate_file_name(id, file_name)
        .await
        .map(Json)
        .map_err(|error| {
            crate::error_codes::store_not_found(
                &error,
                "collector.candidate_not_found",
                "Link not found",
            )
        })
}
