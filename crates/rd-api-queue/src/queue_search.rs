//! The queue's packages and files by name (RD-1240-14), what the search palette lists beside the
//! views and settings and jumps to.
//!
//! Bounded on the server: the palette asks on every pause in typing, and a queue of thousands of
//! files must not travel for the eight rows it shows.

use axum::{Json, extract::State};
use rd_core::{DownloadId, DownloadState, PackageId, PackageState};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{ApiError, AppState};

/// Rows of each kind when the request names no `limit`.
pub const QUEUE_SEARCH_DEFAULT_LIMIT: u32 = 8;
/// The most rows of each kind one answer holds.
pub const QUEUE_SEARCH_MAX_LIMIT: u32 = 50;
/// The longest search text, in characters.
const QUEUE_SEARCH_MAX_TERM: usize = 200;

/// The query string of `GET /api/v1/queue/search`.
#[derive(Clone, Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct QueueSearchQuery {
    /// Part of a package or file name, case-insensitive for ASCII letters; blank finds nothing.
    pub q: Option<String>,
    /// Rows of each kind, 1 to 50; 8 when left out.
    pub limit: Option<u32>,
}

/// `400` for a search text over 200 characters or a limit outside 1 to 50.
#[must_use]
pub fn queue_search_invalid() -> ApiError {
    ApiError::bad_request(
        "queue.search_invalid",
        "The search needs at most 200 characters and a limit from 1 to 50",
    )
    .with_param("max", QUEUE_SEARCH_MAX_LIMIT)
}

/// The query, read so that a value that does not parse is answered with a code.
#[derive(Clone, Debug, Default)]
pub struct QueueSearchParams(pub QueueSearchQuery);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for QueueSearchParams {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<QueueSearchQuery>::try_from_uri(&parts.uri)
            .map(|axum::extract::Query(query)| Self(query))
            .map_err(|_| queue_search_invalid())
    }
}

/// A package whose name matched.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct QueueSearchPackageHit {
    pub id: PackageId,
    pub name: String,
    pub state: PackageState,
}

/// A file whose name matched, with the package it belongs to.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct QueueSearchDownloadHit {
    pub id: DownloadId,
    pub package_id: PackageId,
    pub package_name: String,
    pub file_name: String,
    pub state: DownloadState,
}

/// What the queue holds under a name, each list in queue order.
#[derive(Clone, Debug, Default, Serialize, ToSchema)]
pub struct QueueSearchResponse {
    pub packages: Vec<QueueSearchPackageHit>,
    pub downloads: Vec<QueueSearchDownloadHit>,
}

/// The queue's packages and files whose name contains `q`, at most `limit` of each.
#[utoipa::path(
    get,
    path = "/api/v1/queue/search",
    tag = "downloads",
    params(QueueSearchQuery),
    responses(
        (status = 200, body = QueueSearchResponse),
        (status = 400, description = "queue.search_invalid"),
    )
)]
pub async fn search_queue(
    State(state): State<AppState>,
    QueueSearchParams(query): QueueSearchParams,
) -> Result<Json<QueueSearchResponse>, ApiError> {
    let term = query.q.unwrap_or_default();
    let limit = query.limit.unwrap_or(QUEUE_SEARCH_DEFAULT_LIMIT);
    if term.chars().count() > QUEUE_SEARCH_MAX_TERM
        || !(1..=QUEUE_SEARCH_MAX_LIMIT).contains(&limit)
    {
        return Err(queue_search_invalid());
    }
    let found = state.database.search_queue(&term, limit).await?;
    Ok(Json(QueueSearchResponse {
        packages: found
            .packages
            .into_iter()
            .map(|package| QueueSearchPackageHit {
                id: package.id,
                name: package.name,
                state: package.state,
            })
            .collect(),
        downloads: found
            .downloads
            .into_iter()
            .map(|download| QueueSearchDownloadHit {
                id: download.id,
                package_id: download.package_id,
                package_name: download.package_name,
                file_name: download.file_name,
                state: download.state,
            })
            .collect(),
    }))
}
