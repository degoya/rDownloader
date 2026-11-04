//! A failed package reset whole (RD-1390-07): everything it demonstrably put on disk goes, and it
//! waits in the queue as the LinkGrabber handed it over.
//!
//! The scheduler does the work (`SchedulerHandle::reset_package`); this answers its refusals
//! with stable codes and adds the one it cannot see, a post-processing run that is under way.
//! Whether the unpacked results go too is the setting `package_reset_delete_extracted`.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::PackageId;
use rd_db::StoreErrorKind;

use crate::{ApiError, AppState, dto::MessageResponse};

#[utoipa::path(
    post,
    path = "/api/v1/packages/{id}/reset",
    tag = "downloads",
    params(("id" = rd_core::PackageId, Path)),
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn reset_package(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
) -> Result<Json<MessageResponse>, ApiError> {
    let package = state
        .database
        .get_package(id)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    let pending = state.queue.extraction.pending().await;
    if let Some(code) = crate::package_clear::postprocess_code(&package, &pending) {
        return Err(crate::package_clear::busy_error(code, &package.name));
    }
    let delete_extracted = crate::settings_store::stored_settings(&state.database)
        .await?
        .package_reset_delete_extracted;
    let count = state
        .scheduler
        .reset_package(id, delete_extracted)
        .await
        .map_err(|error| reset_refusal(error, &package.name))?;
    Ok(Json(
        MessageResponse::new(
            "package.reset",
            format!("Package reset: {count} file(s) start again from the beginning"),
        )
        .with_count(count),
    ))
}

/// Maps the scheduler's refusals onto the codes the interface translates.
fn reset_refusal(error: anyhow::Error, name: &str) -> ApiError {
    if error
        .downcast_ref::<rd_scheduler::PackageNotFailed>()
        .is_some()
    {
        return ApiError::conflict(
            "package.reset_not_failed",
            "Only a failed package can be reset completely",
        )
        .with_param("name", name);
    }
    if error.downcast_ref::<rd_scheduler::NzbDropped>().is_some() {
        return ApiError::conflict(
            "download.nzb_dropped",
            "The NZB of this Usenet download was dropped, so it cannot be fetched again",
        );
    }
    match rd_db::store_kind(&error) {
        Some(StoreErrorKind::WrongState) => ApiError::conflict(
            "package.reset_running",
            "Pause or stop the running files of this package first",
        )
        .with_param("name", name),
        Some(StoreErrorKind::NotFound) => crate::error_codes::package_not_found(),
        _ => error.into(),
    }
}
