//! Pausing and resuming the whole queue from the capture agent's tray (RD-1100-06).
//!
//! An agent's token reaches these two routes only when it was paired with queue control
//! (`capture:queue`, checked by `rd_api_core::auth::require_capture_queue`); a plain capture
//! token is refused with `auth.scope_insufficient`. They do what the web interface's global
//! control does: "pause all" stops every waiting and moving file, a duration makes it the timed
//! pause of RD-190-20, and "resume all" ends a timed pause or, without one, queues the paused
//! files again. The answers are counts and a time, like the summary beside them: nothing names a
//! file.

use axum::{Json, extract::State};
use chrono::{DateTime, Utc};
use rd_core::DownloadState;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    queue_pause_handlers::{QueuePauseRequest, pause_end},
};

#[derive(Deserialize, ToSchema)]
pub struct CaptureQueuePauseRequest {
    /// How long, in minutes from now, within thirty days; absent or `null` pauses until resumed.
    #[serde(default)]
    pub minutes: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct CaptureQueueResponse {
    /// Files this request paused or queued again.
    pub files: u32,
    /// When the timed pause ends, while one holds.
    pub paused_until: Option<DateTime<Utc>>,
}

/// Pauses every waiting and moving file; with `minutes`, until then, holding back new ones.
#[utoipa::path(post, path = "/api/v1/capture/queue/pause", tag = "capture", request_body = CaptureQueuePauseRequest, responses((status = 200, body = CaptureQueueResponse), (status = 400, body = crate::error::ErrorBody), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn pause_capture_queue(
    State(state): State<AppState>,
    Json(request): Json<CaptureQueuePauseRequest>,
) -> Result<Json<CaptureQueueResponse>, ApiError> {
    if let Some(minutes) = request.minutes {
        let timed = QueuePauseRequest {
            minutes: Some(minutes),
            until: None,
        };
        let pause = state
            .scheduler
            .pause_queue_until(pause_end(&timed, Utc::now())?)
            .await?;
        return Ok(Json(CaptureQueueResponse {
            files: count(pause.files.len()),
            paused_until: Some(pause.until),
        }));
    }
    let mut paused = 0;
    for file in state.database.list_downloads().await? {
        if !rd_scheduler::pausable(file.state) {
            continue;
        }
        // A file that finished or was removed a moment ago has nothing left to pause.
        match state.scheduler.pause(file.id).await {
            Ok(()) => paused += 1,
            Err(error) => {
                tracing::debug!(%error, download = %file.id, "a file could not be paused from the tray");
            }
        }
    }
    Ok(Json(CaptureQueueResponse {
        files: count(paused),
        paused_until: state.scheduler.queue_pause().await.map(|pause| pause.until),
    }))
}

/// Ends a timed pause, which queues the files it stopped again; without one, queues every
/// paused file again. A failed or cancelled file stays as it is: the tray resumes what a pause
/// stopped, and restarting what failed is the web interface's decision.
#[utoipa::path(post, path = "/api/v1/capture/queue/resume", tag = "capture", responses((status = 200, body = CaptureQueueResponse), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn resume_capture_queue(
    State(state): State<AppState>,
) -> Result<Json<CaptureQueueResponse>, ApiError> {
    // A start by hand outranks the hold of an account whose traffic is used up (RD-1190-14).
    state.scheduler.release_account_traffic().await;
    if state.scheduler.queue_pause().await.is_some() {
        let resumed = state.scheduler.resume_queue().await?;
        return Ok(Json(CaptureQueueResponse {
            files: count(resumed),
            paused_until: None,
        }));
    }
    let mut resumed = 0;
    for file in state.database.list_downloads().await? {
        if file.state != DownloadState::Paused {
            continue;
        }
        match state.scheduler.resume(file.id).await {
            Ok(()) => resumed += 1,
            Err(error) => {
                tracing::debug!(%error, download = %file.id, "a file could not be resumed from the tray");
            }
        }
    }
    Ok(Json(CaptureQueueResponse {
        files: count(resumed),
        paused_until: None,
    }))
}

fn count(files: usize) -> u32 {
    u32::try_from(files).unwrap_or(u32::MAX)
}
