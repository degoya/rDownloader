//! A package's "not before" (RD-1240-14): its files start no earlier than a chosen moment.
//!
//! The scheduler's dispatch pass leaves the waiting files of such a package alone until then
//! (`rd-scheduler`'s `dispatch.rs`); running files go on, and a file started by hand waits like
//! the rest. A moment that is not in the future holds nothing, so it is stored as none.

use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use rd_core::PackageId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState};

/// Sets or removes a package's "not before".
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default)]
pub struct PackageStartAfterRequest {
    /// The moment (RFC 3339) the package's files may start from; `null`, or a moment that has
    /// passed, removes it.
    pub start_after: Option<DateTime<Utc>>,
}

/// A package's "not before" as stored.
#[derive(Debug, Serialize, ToSchema)]
pub struct PackageStartAfterResponse {
    pub package_id: PackageId,
    /// `null` when the package starts as the queue reaches it.
    pub start_after: Option<DateTime<Utc>>,
}

/// Sets or removes the moment a package's files may start from; the next dispatch pass follows.
#[utoipa::path(
    put,
    path = "/api/v1/packages/{id}/start-after",
    tag = "downloads",
    params(("id" = rd_core::PackageId, Path)),
    request_body = PackageStartAfterRequest,
    responses(
        (status = 200, body = PackageStartAfterResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn set_package_start_after(
    State(state): State<AppState>,
    Path(id): Path<PackageId>,
    Json(request): Json<PackageStartAfterRequest>,
) -> Result<Json<PackageStartAfterResponse>, ApiError> {
    let start_after = request.start_after.filter(|at| *at > Utc::now());
    if !state
        .database
        .set_package_start_after(id, start_after)
        .await?
    {
        return Err(crate::error_codes::package_not_found());
    }
    Ok(Json(PackageStartAfterResponse {
        package_id: id,
        start_after,
    }))
}
