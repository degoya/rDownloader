//! The LinkGrabber's batches and single links: listing, removing and enqueueing one.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use rd_api_core::list_bounds::paged;
use rd_core::CandidateId;
use rd_db::StoreErrorKind;

use crate::{
    ApiError, AppState,
    dto::{MessageResponse, PageQuery},
};

/// Every capture batch, newest first; `limit`/`offset` cut a page out of that order (API-15).
#[utoipa::path(get, path = "/api/v1/collector/batches", tag = "collector", params(PageQuery), responses((status = 200, body = [rd_core::CollectorBatch], headers(("x-total-count" = u64, description = "How many rows the whole list holds; sent only when `limit` or `offset` asked for a page"))), (status = 400)))]
pub async fn list_batches(
    State(state): State<AppState>,
    rd_api_core::list_bounds::Page(page): rd_api_core::list_bounds::Page,
) -> Result<(HeaderMap, Json<Vec<rd_core::CollectorBatch>>), ApiError> {
    let window = page.window()?;
    Ok(paged(
        window,
        state.database.list_collector_batches().await?,
    ))
}

/// Every link not yet enqueued, in package and link order; `limit`/`offset` cut a page out of
/// that order (API-15).
#[utoipa::path(get, path = "/api/v1/collector/candidates", tag = "collector", params(PageQuery), responses((status = 200, body = [rd_core::LinkCandidate], headers(("x-total-count" = u64, description = "How many rows the whole list holds; sent only when `limit` or `offset` asked for a page"))), (status = 400)))]
pub async fn list_candidates(
    State(state): State<AppState>,
    rd_api_core::list_bounds::Page(page): rd_api_core::list_bounds::Page,
) -> Result<(HeaderMap, Json<Vec<rd_core::LinkCandidate>>), ApiError> {
    let window = page.window()?;
    Ok(paged(window, state.database.list_candidates().await?))
}

#[utoipa::path(delete, path = "/api/v1/collector/candidates/{id}", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn delete_candidate(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .delete_candidate(id)
        .await
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::NotFound) => {
                ApiError::not_found("collector.candidate_not_found", "Link not found")
            }
            Some(StoreErrorKind::Busy) => ApiError::conflict(
                "collector.candidate_busy",
                "Link is being enqueued and cannot be deleted",
            ),
            _ => error.into(),
        })?;
    crate::torrent_intake::prune_checked_torrents(&state).await;
    Ok(Json(MessageResponse::new(
        "collector.candidate_removed",
        "Link removed from the LinkGrabber",
    )))
}

#[utoipa::path(delete, path = "/api/v1/collector/candidates", tag = "collector", responses((status = 200, body = MessageResponse)))]
pub async fn delete_candidates(
    State(state): State<AppState>,
) -> Result<Json<MessageResponse>, ApiError> {
    let count = state.database.delete_candidates().await?;
    crate::torrent_intake::prune_checked_torrents(&state).await;
    Ok(Json(
        MessageResponse::new(
            "collector.candidates_removed",
            format!("{count} link(s) removed from the LinkGrabber"),
        )
        .with_count(count),
    ))
}

#[utoipa::path(post, path = "/api/v1/collector/candidates/{id}/enqueue", tag = "collector", params(("id" = String, Path)), responses((status = 201, body = rd_core::DownloadPackage), (status = 409)))]
pub async fn enqueue_candidate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<rd_core::DownloadPackage>), ApiError> {
    let id = crate::error_codes::parse_id::<CandidateId>(&id)?;
    let candidate =
        state.database.get_candidate(id).await?.ok_or_else(|| {
            ApiError::not_found("collector.candidate_not_found", "Link not found")
        })?;
    // `LinkCandidateState::ENQUEUEABLE` is the one list; the package path builds its SQL from
    // the same constant. The two used to disagree — a single `unsupported` link was refused
    // while the same link inside a package went through.
    if !candidate.state.is_enqueueable() {
        return Err(ApiError::conflict(
            "collector.candidate_not_enqueueable",
            "This link has already been enqueued or is being processed",
        ));
    }
    // A single link becomes its own package (moved out of its group first).
    let package = state
        .database
        .move_candidates(
            vec![id],
            rd_db::MoveTarget::New {
                // The package name doubles as the folder name, so file extensions are stripped.
                name: candidate.file_name.clone().map_or_else(
                    || candidate.url.host_str().unwrap_or("Link").to_owned(),
                    |name| rd_files::package_name_from_file_name(&name),
                ),
            },
        )
        .await?;
    let created =
        crate::collector_enqueue::enqueue_package(&state, package.id, false, None).await?;
    Ok((StatusCode::CREATED, Json(created.package)))
}
