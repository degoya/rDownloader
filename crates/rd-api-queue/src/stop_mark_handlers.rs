//! The queue's stop mark (RD-1210-02): set it on a file or a package, read it, remove it.
//!
//! The mark is the scheduler's (`crates/rd-scheduler/src/stop_mark.rs`): it holds the queue once
//! its target is done, through the same pause `/api/v1/queue/pause` reads and ends. These routes
//! only place it; reading the pause names it too, so one read tells why the queue will stop.

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use rd_core::{DownloadId, PackageId};
use rd_db::{StopMark, StopMarkTarget, StoreErrorKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

#[derive(Deserialize, ToSchema)]
pub struct QueueStopMarkRequest {
    /// The file to stop after; give this or `package_id`.
    #[serde(default)]
    pub download_id: Option<DownloadId>,
    /// The package to stop after, once none of its files waits or runs; give this or
    /// `download_id`.
    #[serde(default)]
    pub package_id: Option<PackageId>,
}

/// The stop mark in force: where the queue will pause.
#[derive(Serialize, ToSchema)]
pub struct QueueStopMarkResponse {
    /// The marked file; `null` when the mark sits on a package.
    pub download_id: Option<DownloadId>,
    /// The marked package; `null` when the mark sits on a file.
    pub package_id: Option<PackageId>,
    /// The file's or the package's name, as the queue shows it.
    pub name: String,
    pub set_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct QueueStopMarkStateResponse {
    /// `null` while no mark is set.
    pub stop_mark: Option<QueueStopMarkResponse>,
}

#[derive(Serialize, ToSchema)]
pub struct QueueStopMarkClearResponse {
    /// Whether a mark was set and is gone now.
    pub cleared: bool,
}

/// The stop mark in force, if any.
#[utoipa::path(get, path = "/api/v1/queue/stop-mark", tag = "downloads", responses((status = 200, body = QueueStopMarkStateResponse)))]
pub async fn get_queue_stop_mark(
    State(state): State<AppState>,
) -> Result<Json<QueueStopMarkStateResponse>, ApiError> {
    Ok(Json(QueueStopMarkStateResponse {
        stop_mark: current(&state).await?,
    }))
}

/// Sets the stop mark on a file or a package, replacing the one in force. Once the file is done
/// (completed or failed for good), or no file of the package waits or runs any more, the queue
/// pauses until it is resumed; what runs at that moment finishes. A target that is already done
/// is refused.
#[utoipa::path(put, path = "/api/v1/queue/stop-mark", tag = "downloads", request_body = QueueStopMarkRequest, responses((status = 200, body = QueueStopMarkResponse), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)))]
pub async fn set_queue_stop_mark(
    State(state): State<AppState>,
    Json(request): Json<QueueStopMarkRequest>,
) -> Result<Json<QueueStopMarkResponse>, ApiError> {
    let target = match (request.download_id, request.package_id) {
        (Some(id), None) => StopMarkTarget::Download(id),
        (None, Some(id)) => StopMarkTarget::Package(id),
        _ => {
            return Err(ApiError::bad_request(
                "queue.stop_mark_target_invalid",
                "Give the stop mark either a download or a package",
            ));
        }
    };
    let mark =
        state
            .scheduler
            .set_stop_mark(target)
            .await
            .map_err(|error| match rd_db::store_kind(&error) {
                Some(StoreErrorKind::NotFound) => match target {
                    StopMarkTarget::Download(_) => crate::error_codes::download_not_found(),
                    StopMarkTarget::Package(_) => crate::error_codes::package_not_found(),
                },
                Some(StoreErrorKind::WrongState) => ApiError::conflict(
                    "queue.stop_mark_target_finished",
                    "That download or package is already finished",
                ),
                _ => ApiError::from(error),
            })?;
    // Gone in the moment between the write and this read: the mark went with it.
    describe(&state, mark)
        .await?
        .map(Json)
        .ok_or_else(|| match target {
            StopMarkTarget::Download(_) => crate::error_codes::download_not_found(),
            StopMarkTarget::Package(_) => crate::error_codes::package_not_found(),
        })
}

/// Removes the stop mark; the queue then runs on past its target.
#[utoipa::path(delete, path = "/api/v1/queue/stop-mark", tag = "downloads", responses((status = 200, body = QueueStopMarkClearResponse)))]
pub async fn clear_queue_stop_mark(
    State(state): State<AppState>,
) -> Result<Json<QueueStopMarkClearResponse>, ApiError> {
    Ok(Json(QueueStopMarkClearResponse {
        cleared: state.scheduler.clear_stop_mark().await?,
    }))
}

/// The mark in force, named; `None` while there is none.
pub(crate) async fn current(state: &AppState) -> Result<Option<QueueStopMarkResponse>, ApiError> {
    match state.scheduler.stop_mark().await? {
        Some(mark) => describe(state, mark).await,
        None => Ok(None),
    }
}

/// A mark with its target's name; `None` when the target is gone, which takes the mark with it.
async fn describe(
    state: &AppState,
    mark: StopMark,
) -> Result<Option<QueueStopMarkResponse>, ApiError> {
    let (download_id, package_id, name) = match mark.target {
        StopMarkTarget::Download(id) => match state.database.get_download(id).await? {
            Some(file) => (Some(id), None, file.file_name),
            None => return Ok(None),
        },
        StopMarkTarget::Package(id) => match state.database.get_package(id).await? {
            Some(package) => (None, Some(id), package.name),
            None => return Ok(None),
        },
    };
    Ok(Some(QueueStopMarkResponse {
        download_id,
        package_id,
        name,
        set_at: mark.set_at,
    }))
}
