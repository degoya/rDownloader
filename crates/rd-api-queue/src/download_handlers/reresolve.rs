//! Resolving downloads again with the plugin installed now (RD-1210-01).
//!
//! A download is pinned to the resolver version that first resolved it, so an update cannot
//! change the rules under a running job. A job that was resolved by a faulty version keeps
//! that version for good, though — a tester saw DDownload files behave like the old plugin
//! after an update. This drops the pin: the next start resolves with whatever plugin the host
//! is assigned to now. A running file is paused first and started again afterwards, since a
//! running job keeps its version; finished files are left as they are.
//!
//! The bytes already on disk are not touched here. The next attempt plans its transfer exactly
//! as every resume does: a size or validator that changed between the recorded transfer and the
//! new resolution stops the file as `blocked (validators-changed)` rather than writing another
//! file's bytes over confirmed ones, and a matching one resumes where it stopped.

use std::time::Duration;

use axum::{Json, extract::State};
use rd_core::{DownloadId, DownloadState};
use rd_db::StoreErrorKind;

use crate::{
    ApiError, AppState,
    audit::{AuditContext, AuditEvent},
    dto::{DownloadBulkResponse, ReresolveRequest},
};
use rd_api_core::list_bounds::validate_bulk;

/// How often, and how far apart, the pin is tried again while a paused worker lets go.
const PAUSE_WAIT_ROUNDS: usize = 25;
const PAUSE_WAIT_STEP: Duration = Duration::from_millis(200);

#[utoipa::path(post, path = "/api/v1/downloads/reresolve", tag = "downloads", request_body = ReresolveRequest, responses((status = 200, body = DownloadBulkResponse, description = "`affected` counts the files that resolve anew at their next start; each refusal is coded"), (status = 400, description = "No download or more than 500 named"), (status = 404, description = "A named package does not exist")))]
pub async fn reresolve_downloads(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<ReresolveRequest>,
) -> Result<Json<DownloadBulkResponse>, ApiError> {
    let ids = selected(&state, request).await?;
    let mut answer = DownloadBulkResponse {
        affected: 0,
        errors: Vec::new(),
        refusals: Vec::new(),
    };
    for id in ids {
        match reresolve(&state, id).await {
            Ok(Some(released)) => {
                answer.affected += 1;
                if let Some(pin) = released {
                    let event = AuditEvent::success(rd_core::AuditAction::PluginVersionChosen)
                        .by(&audit)
                        .target("plugin", pin.plugin_id)
                        .detail("choice", "reresolve")
                        .detail("version", pin.version)
                        .detail("download_id", id);
                    crate::audit::record(&state, event).await;
                }
            }
            Ok(None) => {}
            Err(error) => {
                answer.errors.push(format!("{id}: {}", error.message()));
                answer.refusals.push(error.into_message());
            }
        }
    }
    Ok(Json(answer))
}

/// The named downloads and every file of the named packages, each once, in that order.
async fn selected(
    state: &AppState,
    request: ReresolveRequest,
) -> Result<Vec<DownloadId>, ApiError> {
    let mut ids = request.ids;
    for package_id in request.package_ids {
        if state.database.get_package(package_id).await?.is_none() {
            return Err(crate::error_codes::package_not_found());
        }
        ids.extend(
            state
                .database
                .downloads_for_package(package_id)
                .await?
                .into_iter()
                .map(|file| file.id),
        );
    }
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(*id));
    validate_bulk(ids.len())?;
    Ok(ids)
}

/// Re-resolves one file: `Some(pin it had)` when it resolves anew at its next start, `None` for
/// a file past the resolver, which has nothing left to resolve.
async fn reresolve(
    state: &AppState,
    id: DownloadId,
) -> Result<Option<Option<rd_core::ResolverPin>>, ApiError> {
    let file = state
        .database
        .get_download(id)
        .await?
        .ok_or_else(crate::error_codes::download_not_found)?;
    // Past the resolver already: the bytes are all there, and verifying, repairing, unpacking
    // or seeding them asks no plugin anything.
    if file.state == DownloadState::Completed
        || (file.state.holds_the_file()
            && !matches!(
                file.state,
                DownloadState::Resolving | DownloadState::Downloading
            ))
    {
        return Ok(None);
    }
    let paused = file.state.holds_the_file();
    if paused {
        state.scheduler.pause(id).await.map_err(refusal)?;
    }
    let mut released = state.database.release_resolver_pin(id).await;
    for _ in 0..PAUSE_WAIT_ROUNDS {
        if !matches!(&released, Err(error) if rd_db::store_kind(error) == Some(StoreErrorKind::WrongState))
        {
            break;
        }
        tokio::time::sleep(PAUSE_WAIT_STEP).await;
        released = state.database.release_resolver_pin(id).await;
    }
    let released = released.map_err(refusal)?;
    if paused {
        state.scheduler.resume(id).await.map_err(refusal)?;
    }
    Ok(Some(released))
}

fn refusal(error: anyhow::Error) -> ApiError {
    match rd_db::store_kind(&error) {
        Some(StoreErrorKind::NotFound) => crate::error_codes::download_not_found(),
        Some(StoreErrorKind::WrongState) => ApiError::conflict(
            "download.reresolve_running",
            "The download did not stop in time to be resolved again; pause it and try again",
        ),
        _ => error.into(),
    }
}
