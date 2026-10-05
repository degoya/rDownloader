//! The two manual actions on a queued torrent's data (RD-1100-10): hash it again against its
//! pieces, and move it to another folder while it keeps its row and its seed.

use std::path::{Path as FsPath, PathBuf};

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_core::{DownloadFile, DownloadId, DownloadKind, DownloadState, StorageRootId};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::{ApiError, AppState, dto::MessageResponse};

/// How long a recheck waits for a running torrent to stop before it gives up.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Where a torrent's files move: a folder below a storage root, in which the package keeps a
/// folder of its own name — as a package queued into a category's folder does.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TorrentMoveRequest {
    pub storage_root_id: StorageRootId,
    /// Relative to the storage root; empty for the root itself.
    #[serde(default)]
    pub relative_path: String,
}

/// Hashes a torrent's data against its pieces again.
///
/// A seed is checked in place and seeds on; what does not verify is fetched again. A running
/// torrent is stopped, checked as it is added again, and runs on. Any other torrent is checked
/// when it starts next. The result shows on the torrent (`recheck` in its detail).
#[utoipa::path(
    post,
    path = "/api/v1/downloads/{id}/torrent/recheck",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 202, body = MessageResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn recheck_torrent(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    let download = torrent_row(&state, id).await?;
    refuse_while_moving(&state, id).await?;
    let now = match download.state {
        DownloadState::Seeding
        | DownloadState::Queued
        | DownloadState::Paused
        | DownloadState::RetryWait
        | DownloadState::Failed
        | DownloadState::Blocked
        | DownloadState::Cancelled => recheck(&state, id).await?,
        DownloadState::Resolving | DownloadState::Downloading => {
            // The runner holds the torrent: it is stopped, the check is asked for, and the row
            // is queued again, so the runner's next add is the check.
            state.scheduler.pause(id).await?;
            wait_until_stopped(&state, id).await?;
            recheck(&state, id).await?;
            state.scheduler.resume(id).await?;
            true
        }
        _ => {
            return Err(ApiError::conflict(
                "torrent.recheck_state",
                "A torrent can be checked while it is queued, running, paused or seeding",
            ));
        }
    };
    let message = if now {
        MessageResponse::new("torrent.recheck_started", "Checking the torrent's data")
    } else {
        MessageResponse::new(
            "torrent.recheck_scheduled",
            "The torrent's data is checked when it starts next",
        )
    };
    Ok((StatusCode::ACCEPTED, Json(message)))
}

/// Moves a seeding or paused torrent's files to another folder.
///
/// Refused up front when the target is not free or its filesystem lacks the space for a copy;
/// otherwise the move runs in the background. Until it ends the torrent is out of the session;
/// afterwards the package names the new folder and a seed seeds from it after one check of its
/// data — or, when the move failed, everything is where it was and `relocation_error` in the
/// torrent's detail says why.
#[utoipa::path(
    post,
    path = "/api/v1/downloads/{id}/torrent/move",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    request_body = TorrentMoveRequest,
    responses(
        (status = 202, body = MessageResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn move_torrent(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
    Json(request): Json<TorrentMoveRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    let download = torrent_row(&state, id).await?;
    refuse_while_moving(&state, id).await?;
    if !matches!(
        download.state,
        DownloadState::Seeding | DownloadState::Paused
    ) {
        return Err(ApiError::conflict(
            "torrent.move_state",
            "Only a seeding or paused torrent can be moved",
        ));
    }
    let stored = crate::torrent_control::require_download_state(&state, id).await?;
    if stored.metadata.is_none() {
        return Err(ApiError::bad_request(
            "torrent.metadata_pending",
            "The torrent metadata has not been resolved yet",
        ));
    }
    let package = state
        .database
        .get_package(download.package_id)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    // The package's folder is what moves; a file of another download in it would be left
    // pointing at a folder its data is no longer in.
    if state
        .database
        .downloads_for_package(package.id)
        .await?
        .iter()
        .any(|other| other.id != id)
    {
        return Err(ApiError::conflict(
            "torrent.move_package_shared",
            "The package holds other downloads too, so its folder cannot follow the torrent",
        ));
    }
    let root = state
        .database
        .list_storage_roots()
        .await?
        .into_iter()
        .find(|root| root.id == request.storage_root_id)
        .ok_or_else(|| ApiError::not_found("storage_root.not_found", "Storage root not found"))?;
    let invalid = |error: anyhow::Error| {
        ApiError::bad_request("torrent.move_target_invalid", format!("{error:#}"))
    };
    let root = rd_files::StorageRoot::create(root.id, root.name, PathBuf::from(root.path))
        .await
        .map_err(invalid)?;
    let base = root
        .resolve(FsPath::new(request.relative_path.trim()))
        .map_err(invalid)?;
    let target = rd_files::package_directory(&base, &package.name);
    let from = PathBuf::from(&package.destination);
    if target.starts_with(&from) || from.starts_with(&target) {
        return Err(ApiError::conflict(
            "torrent.move_same_place",
            "The torrent is already there, or the new folder lies inside its own",
        ));
    }
    let destination = target.to_string_lossy().into_owned();
    let taken = !folder_is_free(&target).await
        || state
            .database
            .list_packages()
            .await?
            .iter()
            .any(|other| other.id != package.id && other.destination == destination);
    if taken {
        return Err(ApiError::conflict(
            "torrent.move_target_exists",
            "A folder of that name is already there",
        )
        .with_param("path", &destination));
    }
    // Within one filesystem the files are renamed and need no space; across two they are
    // copied, and the copy must fit and leave the root's free-space threshold.
    if rd_files::same_file_system(&from, root.path()) != Some(true) {
        let required = state.torrent.relocation_bytes(id).await?;
        let verdict = state
            .scheduler
            .capacity()
            .check(root.path(), Some(required))
            .await?;
        if let Some(shortfall) = verdict.shortfall() {
            return Err(ApiError::conflict(
                "torrent.move_no_space",
                "There is not enough free space for the torrent's files there",
            )
            .with_param("required_bytes", shortfall.required_bytes)
            .with_param("free_bytes", shortfall.free_bytes));
        }
    }
    state
        .torrent
        .start_relocation(id, target)
        .await
        .map_err(|error| ApiError::bad_request("torrent.move_failed", format!("{error:#}")))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(MessageResponse::new(
            "torrent.move_started",
            "Moving the torrent's files",
        )),
    ))
}

/// The queue row, refused unless it is a torrent.
async fn torrent_row(state: &AppState, id: DownloadId) -> Result<DownloadFile, ApiError> {
    let download = state
        .database
        .get_download(id)
        .await?
        .ok_or_else(crate::error_codes::download_not_found)?;
    if download.kind != DownloadKind::Torrent {
        return Err(ApiError::bad_request(
            "torrent.not_a_torrent",
            "This download is not a torrent",
        ));
    }
    Ok(download)
}

/// Neither action runs while the torrent's files are on their way to another folder.
async fn refuse_while_moving(state: &AppState, id: DownloadId) -> Result<(), ApiError> {
    if state.torrent.is_relocating(id).await {
        return Err(ApiError::conflict(
            "torrent.relocation_running",
            "The torrent's files are being moved",
        ));
    }
    Ok(())
}

async fn recheck(state: &AppState, id: DownloadId) -> Result<bool, ApiError> {
    state
        .torrent
        .recheck(id)
        .await
        .map_err(|error| ApiError::bad_request("torrent.recheck_failed", format!("{error:#}")))
}

/// Waits until a paused runner has let go of the torrent.
async fn wait_until_stopped(state: &AppState, id: DownloadId) -> Result<(), ApiError> {
    let deadline = tokio::time::Instant::now() + STOP_WAIT;
    loop {
        let current = state
            .database
            .get_download(id)
            .await?
            .ok_or_else(crate::error_codes::download_not_found)?;
        if !matches!(
            current.state,
            DownloadState::Resolving | DownloadState::Downloading
        ) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ApiError::conflict(
                "torrent.recheck_busy",
                "The torrent did not stop in time; try again",
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Whether `path` is no folder yet or an empty one.
async fn folder_is_free(path: &FsPath) -> bool {
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => matches!(entries.next_entry().await, Ok(None)),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}
