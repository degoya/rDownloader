//! A package's own download window (RD-1240-30): the weekly times its files may download, and
//! whether they download while the bandwidth schedule pauses downloads.
//!
//! The scheduler applies it on its next dispatch pass and within seconds to running transfers
//! (`rd-scheduler`'s `download_window.rs`). Removing it lets the package follow its category's.
//! The read says what holds the package back right now.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_api_core::download_window_input::{DownloadWindowRequest, validated_download_window};
use rd_core::{DownloadWindow, PackageId};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{ApiError, AppState};

/// A package's own download window as stored.
#[derive(Debug, Serialize, ToSchema)]
pub struct PackageDownloadWindowResponse {
    pub package_id: PackageId,
    /// `null` when the package follows its category's window.
    pub download_window: Option<DownloadWindow>,
}

/// A package's download window and what it does right now.
#[derive(Debug, Serialize, ToSchema)]
pub struct PackageDownloadWindowStatus {
    pub package_id: PackageId,
    /// The package's own window; `null` when it follows its category's.
    pub download_window: Option<DownloadWindow>,
    /// The window of the package's category, which applies while the package has none.
    pub category_window: Option<DownloadWindow>,
    /// Why the package's files wait right now: `window` (outside its window) or `schedule` (the
    /// bandwidth profile in force pauses downloads); `null` while they may download.
    pub held: Option<rd_limits::DownloadHold>,
}

/// Reads a package's download window, its category's, and whether either or the schedule holds
/// the package back now.
#[utoipa::path(
    get,
    path = "/api/v1/packages/{id}/download-window",
    tag = "downloads",
    params(("id" = rd_core::PackageId, Path)),
    responses(
        (status = 200, body = PackageDownloadWindowStatus),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn get_package_download_window(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
) -> Result<Json<PackageDownloadWindowStatus>, ApiError> {
    let package = state
        .database
        .get_package(id)
        .await?
        .ok_or_else(crate::error_codes::package_not_found)?;
    let category_window = match package.category_id {
        Some(category) => state
            .database
            .list_categories()
            .await?
            .into_iter()
            .find(|candidate| candidate.id == category)
            .and_then(|candidate| candidate.download_window),
        None => None,
    };
    let held = state.scheduler.package_download_hold(&package).await?;
    Ok(Json(PackageDownloadWindowStatus {
        package_id: id,
        download_window: package.download_window,
        category_window,
        held,
    }))
}

/// Sets or removes a package's own download window; it never makes the package faster than the
/// global, profile or hand-set limits.
#[utoipa::path(
    put,
    path = "/api/v1/packages/{id}/download-window",
    tag = "downloads",
    params(("id" = rd_core::PackageId, Path)),
    request_body = DownloadWindowRequest,
    responses(
        (status = 200, body = PackageDownloadWindowResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn set_package_download_window(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
    Json(request): Json<DownloadWindowRequest>,
) -> Result<Json<PackageDownloadWindowResponse>, ApiError> {
    let download_window = validated_download_window(request.download_window)?;
    if !state
        .database
        .set_package_download_window(id, download_window.clone())
        .await?
    {
        return Err(crate::error_codes::package_not_found());
    }
    Ok(Json(PackageDownloadWindowResponse {
        package_id: id,
        download_window,
    }))
}
