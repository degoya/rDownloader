use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use rd_core::DownloadId;
use rd_db::StoreErrorKind;
use url::Url;

use crate::{
    ApiError, AppState,
    dto::{
        CreateDownloadRequest, DownloadBulkAction, DownloadBulkRequest, DownloadBulkResponse,
        DownloadExtractRequest, DownloadRateEntry, DownloadRatesResponse, DownloadRenameRequest,
        DownloadSummaryResponse, MessageResponse, PageQuery, StorageSpace,
    },
    error_codes::parse_id,
};
use rd_api_core::list_bounds::{paged, validate_bulk};

mod create;
mod summary;

pub use create::*;
pub use summary::*;

/// Every download in creation order; `limit`/`offset` cut a page out of that order (API-15).
#[utoipa::path(get, path = "/api/v1/downloads", tag = "downloads", params(PageQuery), responses((status = 200, body = [rd_core::DownloadFile], headers(("x-total-count" = u64, description = "How many rows the whole list holds; sent only when `limit` or `offset` asked for a page"))), (status = 400)))]
pub async fn list_downloads(
    State(state): State<AppState>,
    rd_api_core::list_bounds::Page(page): rd_api_core::list_bounds::Page,
) -> Result<(HeaderMap, Json<Vec<rd_core::DownloadFile>>), ApiError> {
    let window = page.window()?;
    Ok(paged(window, state.database.list_downloads().await?))
}

#[utoipa::path(patch, path = "/api/v1/downloads/{id}", tag = "downloads", params(("id" = String, Path)), request_body = DownloadRenameRequest, responses((status = 200, body = rd_core::DownloadFile), (status = 404), (status = 409)))]
pub async fn rename_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<DownloadRenameRequest>,
) -> Result<Json<rd_core::DownloadFile>, ApiError> {
    let file_name = rd_files::sanitize_file_name(request.file_name.trim());
    if file_name.is_empty() || file_name.chars().count() > 255 {
        return Err(ApiError::bad_request(
            "download.file_name_length",
            "File name must be between 1 and 255 characters",
        )
        .with_param("max", 255));
    }
    state
        .database
        .rename_download(parse_id::<DownloadId>(&id)?, file_name)
        .await
        .map(Json)
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::WrongState) => ApiError::conflict(
                "download.rename_state",
                "Only queued, paused or failed downloads can be renamed",
            ),
            Some(StoreErrorKind::NotFound) => crate::error_codes::download_not_found(),
            _ => ApiError::from(error),
        })
}

#[utoipa::path(post, path = "/api/v1/downloads/bulk", tag = "downloads", request_body = DownloadBulkRequest, responses((status = 200, body = DownloadBulkResponse)))]
pub async fn bulk_downloads(
    State(state): State<AppState>,
    Json(request): Json<DownloadBulkRequest>,
) -> Result<Json<DownloadBulkResponse>, ApiError> {
    Ok(Json(
        apply_download_action(&state, request.action, request.ids).await?,
    ))
}

pub async fn apply_download_action(
    state: &AppState,
    action: DownloadBulkAction,
    ids: Vec<DownloadId>,
) -> Result<DownloadBulkResponse, ApiError> {
    validate_bulk(ids.len())?;
    let mut affected = 0_u32;
    let mut errors = Vec::new();
    let mut refusals = Vec::new();
    for id in ids {
        let result = match action {
            DownloadBulkAction::Pause => state.scheduler.pause(id).await,
            DownloadBulkAction::Resume => state.scheduler.resume(id).await,
            DownloadBulkAction::Cancel => state.scheduler.cancel(id).await,
            DownloadBulkAction::Remove => remove_with_cancel(state, id, false).await,
            DownloadBulkAction::Reset => reset_download_file(state, id, false).await,
            DownloadBulkAction::ResetDeleteFiles => reset_download_file(state, id, true).await,
        };
        match result {
            Ok(()) => affected += 1,
            Err(error) => {
                errors.push(format!("{id}: {error}"));
                refusals.push(bulk_refusal(action, error).into_message());
            }
        }
    }
    Ok(DownloadBulkResponse {
        affected,
        errors,
        refusals,
    })
}

/// The coded refusal of one file in a batch, the same code its single endpoint answers with.
fn bulk_refusal(action: DownloadBulkAction, error: anyhow::Error) -> ApiError {
    if refused(&error, StoreErrorKind::NotFound) {
        return crate::error_codes::download_not_found();
    }
    if error
        .downcast_ref::<rd_scheduler::mirrors::MirrorTaken>()
        .is_some()
    {
        return ApiError::conflict(
            "download.mirror_active",
            "Another link to this file is already downloading",
        );
    }
    if error.downcast_ref::<rd_scheduler::NzbDropped>().is_some() {
        return nzb_dropped();
    }
    if !refused(&error, StoreErrorKind::WrongState) {
        return error.into();
    }
    match action {
        DownloadBulkAction::Cancel => ApiError::conflict(
            "download.cancel_state",
            "The download cannot be cancelled in its current state",
        ),
        DownloadBulkAction::Remove => ApiError::conflict(
            "download.active_must_pause",
            "Active downloads must be cancelled or paused before removal",
        ),
        DownloadBulkAction::Reset | DownloadBulkAction::ResetDeleteFiles => reset_failure(error),
        DownloadBulkAction::Pause => ApiError::conflict(
            "download.pause_state",
            "The download cannot be paused in its current state",
        ),
        DownloadBulkAction::Resume => ApiError::conflict(
            "download.resume_state",
            "The download cannot be resumed in its current state",
        ),
    }
}

/// Discards a file's data and queues it again from zero.
///
/// A torrent additionally leaves the persisted librqbit session, because a handle that still
/// knows the old pieces would resume them instead of fetching the data again.
pub(crate) async fn reset_download_file(
    state: &AppState,
    id: DownloadId,
    delete_completed_files: bool,
) -> anyhow::Result<()> {
    state.scheduler.reset(id, delete_completed_files).await?;
    state.torrent.forget(id).await;
    Ok(())
}

/// Removes one queue row, and with it the row's torrent from the engine session.
///
/// The one removal path: the single `DELETE`, the bulk action, package deletion — and through
/// that auto-remove, the SABnzbd and qBittorrent compatibility APIs and MCP — all end here.
/// Only the single `DELETE` used to forget the torrent, so a torrent removed any other way
/// stayed in the persisted librqbit session and had its files created again on every start
/// (RD-120-68). The torrent is located first because its info hash is stored on the row.
/// The payload stays on disk, as for every other kind of download.
pub(crate) async fn remove_download(state: &AppState, id: DownloadId) -> anyhow::Result<()> {
    remove_download_discarding(state, id, false).await
}

/// [`remove_download`], and with `discard_partial` the data an unfinished file wrote outside
/// staging goes first: tool fragments beside the target, an unfinished torrent's files
/// (RD-180-21). Data first, row last — a crash in between leaves a row that knows less data,
/// never data that no row knows. The caller decides what counts as unfinished.
async fn remove_download_discarding(
    state: &AppState,
    id: DownloadId,
    discard_partial: bool,
) -> anyhow::Result<()> {
    let torrent = state.torrent.locate(id).await;
    if discard_partial {
        state.scheduler.discard_partial(id).await?;
        state.torrent.discard_located(torrent.clone()).await;
    }
    state.scheduler.remove(id).await?;
    state.torrent.forget_located(torrent).await;
    Ok(())
}

/// Cancels an active file first and waits briefly for its token to clear before removal.
///
/// `discard_partial` as in [`remove_download_discarding`].
pub(crate) async fn remove_with_cancel(
    state: &AppState,
    id: DownloadId,
    discard_partial: bool,
) -> anyhow::Result<()> {
    if let Some(current) = state.database.get_download(id).await?
        && matches!(
            current.state,
            rd_core::DownloadState::Resolving
                | rd_core::DownloadState::Downloading
                | rd_core::DownloadState::Verifying
                | rd_core::DownloadState::Repairing
                | rd_core::DownloadState::Extracting
        )
    {
        state.scheduler.cancel(id).await?;
        for _ in 0..25 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if remove_download_discarding(state, id, discard_partial)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
    }
    remove_download_discarding(state, id, discard_partial).await
}

#[utoipa::path(post, path = "/api/v1/downloads/extract", tag = "downloads", request_body = DownloadExtractRequest, responses((status = 202, body = MessageResponse), (status = 409)))]
pub async fn extract_downloads(
    State(state): State<AppState>,
    Json(request): Json<DownloadExtractRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    validate_bulk(request.ids.len())?;
    let downloads = state.database.list_downloads().await?;
    let mut packages: Vec<rd_core::PackageId> = downloads
        .iter()
        .filter(|file| {
            request.ids.contains(&file.id) && file.state == rd_core::DownloadState::Completed
        })
        .map(|file| file.package_id)
        .collect();
    packages.sort();
    packages.dedup();
    if packages.is_empty() {
        return Err(ApiError::conflict(
            "download.none_completed",
            "None of the selected files is completed",
        ));
    }
    let count = packages.len();
    for package_id in packages {
        state
            .extraction
            .request(package_id, rd_extract::ExtractionTrigger::Manual)
            .await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(
            MessageResponse::new(
                "package.extract_queued",
                format!("Extraction queued for {count} package(s)"),
            )
            .with_count(count),
        ),
    ))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/pause", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn pause_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .scheduler
        .pause(parse_id::<DownloadId>(&id)?)
        .await
        .map_err(|error| bulk_refusal(DownloadBulkAction::Pause, error))?;
    Ok(message("download.paused", "Download paused"))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/resume", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn resume_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .scheduler
        .resume(parse_id::<DownloadId>(&id)?)
        .await
        .map_err(|error| bulk_refusal(DownloadBulkAction::Resume, error))?;
    Ok(message("download.resumed", "Download resumed"))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/cancel", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn cancel_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .scheduler
        .cancel(parse_id::<DownloadId>(&id)?)
        .await
        .map_err(|error| {
            if refused(&error, StoreErrorKind::WrongState) {
                ApiError::conflict(
                    "download.cancel_state",
                    "The download cannot be cancelled in its current state",
                )
            } else if refused(&error, StoreErrorKind::NotFound) {
                crate::error_codes::download_not_found()
            } else {
                error.into()
            }
        })?;
    Ok(message("download.cancelled", "Download cancelled"))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/reset", tag = "downloads", params(("id" = String, Path)), request_body = crate::dto::DownloadResetRequest, responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn reset_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<crate::dto::DownloadResetRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let id = parse_id::<DownloadId>(&id)?;
    reset_download_file(&state, id, request.delete_completed_files)
        .await
        .map_err(reset_failure)?;
    Ok(message(
        "download.reset",
        "Download reset and queued again from the start",
    ))
}

/// A reset of a Usenet file whose NZB its package dropped: nothing left to fetch it from.
fn nzb_dropped() -> ApiError {
    ApiError::conflict(
        "download.nzb_dropped",
        "The NZB of this Usenet download was dropped, so it cannot be fetched again",
    )
}

/// Recognises a refusal from either layer of the reset and remove paths.
///
/// `rd-scheduler` checks the same two conditions before the store is ever reached, so it tags
/// its own refusals with the same reason `rd-db` uses. Neither layer's wording is read here,
/// which is the whole point: a reworded bail in either crate used to turn these documented
/// `409`s into `500`s with nothing to catch it.
fn refused(error: &anyhow::Error, kind: StoreErrorKind) -> bool {
    rd_db::store_kind(error) == Some(kind)
}

/// Maps the refusals of the reset path onto the codes the UI translates.
fn reset_failure(error: anyhow::Error) -> ApiError {
    if error.downcast_ref::<rd_scheduler::NzbDropped>().is_some() {
        return nzb_dropped();
    }
    if refused(&error, StoreErrorKind::WrongState) {
        ApiError::conflict(
            "download.active_must_pause",
            "Active downloads must be cancelled or paused before they can be reset",
        )
    } else if refused(&error, StoreErrorKind::NotFound) {
        crate::error_codes::download_not_found()
    } else {
        error.into()
    }
}

#[utoipa::path(delete, path = "/api/v1/downloads/{id}", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse), (status = 409)))]
pub async fn delete_download(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    let id = parse_id::<DownloadId>(&id)?;
    remove_download(&state, id).await.map_err(|error| {
        if refused(&error, StoreErrorKind::WrongState) {
            ApiError::conflict(
                "download.active_must_pause",
                "Active downloads must be cancelled or paused before removal",
            )
        } else if refused(&error, StoreErrorKind::NotFound) {
            crate::error_codes::download_not_found()
        } else {
            error.into()
        }
    })?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::DownloadDeleted)
            .by(&audit)
            .target("download", id),
    )
    .await;
    Ok(message(
        "download.removed",
        "Download removed from the download list",
    ))
}

fn message(code: &str, value: &str) -> Json<MessageResponse> {
    Json(MessageResponse::new(code, value))
}

#[utoipa::path(put, path = "/api/v1/downloads/{id}/auth-profile", tag = "downloads", params(("id" = rd_core::DownloadId, Path)), request_body = crate::dto::SetDownloadAuthProfileRequest, responses((status = 200, body = rd_core::DownloadFile), (status = 404)))]
pub async fn set_download_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<rd_core::DownloadId>,
    Json(request): Json<crate::dto::SetDownloadAuthProfileRequest>,
) -> Result<Json<rd_core::DownloadFile>, ApiError> {
    // A pinned profile must exist; silently storing a dangling id would only surface as a
    // failure at the next download attempt.
    if let rd_core::AuthProfileSelection::Pinned(profile_id) = request.auth_profile
        && state.database.auth_profile(profile_id).await?.is_none()
    {
        return Err(ApiError::not_found(
            "authprofile.not_found",
            "Auth profile not found",
        ));
    }
    state
        .database
        .set_download_auth_profile(id, request.auth_profile)
        .await
        .map_err(|_| crate::error_codes::download_not_found())?;
    state
        .database
        .get_download(id)
        .await?
        .map(Json)
        .ok_or_else(crate::error_codes::download_not_found)
}

#[cfg(test)]
#[path = "download_handlers_tests.rs"]
pub(crate) mod tests;
