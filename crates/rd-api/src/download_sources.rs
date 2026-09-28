//! The sources of a download that came with a mirror set (RD-150-03).
//!
//! Read-only: the order is fixed when the download is created and the health is the queue's
//! to keep. An address is shown redacted, like every address the interface shows, and the
//! piece hashes are summarised rather than listed — sixty thousand hex strings help nobody.

use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{ApiError, AppState};

/// One source of a download, as the interface shows it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DownloadSourceView {
    /// Place in the order the sources are tried, from zero.
    pub position: u32,
    /// The address, with credentials and signed query values replaced.
    pub url: String,
    pub host: Option<String>,
    pub protocol: rd_core::SourceProtocol,
    /// Lower is preferred; absent when the document ranked it not at all.
    pub priority: Option<u32>,
    /// ISO 3166-1 alpha-2 country code the document gave.
    pub location: Option<String>,
    pub state: rd_core::SourceState,
    pub failures: u32,
    pub backoff_until: Option<DateTime<Utc>>,
    /// Stable code of why the source is out for good.
    pub isolated_code: Option<String>,
    /// Stable code of the most recent failure.
    pub last_error_code: Option<String>,
    pub delivered_bytes: u64,
}

/// What the piece hashes of a download cover, without the hashes.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PieceHashSummary {
    pub algorithm: rd_core::ChecksumAlgorithm,
    pub piece_length: u64,
    pub pieces: usize,
}

/// Every source of a download and what its bytes are checked against.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DownloadSourcesResponse {
    /// Empty for a download that came with a single address.
    pub sources: Vec<DownloadSourceView>,
    pub piece_hashes: Option<PieceHashSummary>,
}

#[utoipa::path(get, path = "/api/v1/downloads/{id}/sources", tag = "downloads", params(("id" = rd_core::DownloadId, Path)), responses((status = 200, body = DownloadSourcesResponse), (status = 404)))]
pub async fn list_download_sources(
    State(state): State<AppState>,
    Path(id): Path<rd_core::DownloadId>,
) -> Result<Json<DownloadSourcesResponse>, ApiError> {
    if state.database.get_download(id).await?.is_none() {
        return Err(crate::error_codes::download_not_found());
    }
    let now = Utc::now();
    let sources = state
        .database
        .download_sources(id)
        .await?
        .into_iter()
        .map(|source| DownloadSourceView {
            state: source.state_at(now),
            url: rd_core::redact_url(&source.url),
            host: source.url.host_str().map(str::to_owned),
            position: source.position,
            protocol: source.protocol,
            priority: source.priority,
            location: source.location,
            failures: source.failures,
            backoff_until: source.backoff_until,
            isolated_code: source.isolated_code,
            last_error_code: source.last_error_code,
            delivered_bytes: source.delivered_bytes,
        })
        .collect();
    let piece_hashes = state
        .database
        .download_piece_hashes(id)
        .await?
        .map(|pieces| PieceHashSummary {
            algorithm: pieces.algorithm,
            piece_length: pieces.length,
            pieces: pieces.hashes.len(),
        });
    Ok(Json(DownloadSourcesResponse {
        sources,
        piece_hashes,
    }))
}
