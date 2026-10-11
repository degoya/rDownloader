//! A category's download window (RD-1240-30): the default for its packages; a package's own
//! wins. Its own route, so the category editor's save never touches it.

use rd_api_core::download_window_input::{DownloadWindowRequest, validated_download_window};
use rd_core::{CategoryId, DownloadWindow};
use serde::Serialize;
use utoipa::ToSchema;

use super::*;

/// A category's download window as stored.
#[derive(Debug, Serialize, ToSchema)]
pub struct CategoryDownloadWindowResponse {
    pub category_id: CategoryId,
    /// `null` when the category's packages follow the bandwidth schedule alone.
    pub download_window: Option<DownloadWindow>,
}

/// Sets or removes the download window of a category's packages; the scheduler reads it on its
/// next pass.
#[utoipa::path(
    put,
    path = "/api/v1/categories/{id}/download-window",
    tag = "configuration",
    params(("id" = rd_core::CategoryId, Path)),
    request_body = DownloadWindowRequest,
    responses(
        (status = 200, body = CategoryDownloadWindowResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn set_category_download_window(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<CategoryId>,
    Json(request): Json<DownloadWindowRequest>,
) -> Result<Json<CategoryDownloadWindowResponse>, ApiError> {
    let download_window = validated_download_window(request.download_window)?;
    if !state
        .database
        .set_category_download_window(id, download_window.clone())
        .await?
    {
        return Err(ApiError::not_found(
            "category.not_found",
            "Category not found",
        ));
    }
    Ok(Json(CategoryDownloadWindowResponse {
        category_id: id,
        download_window,
    }))
}
