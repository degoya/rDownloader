//! Tracker inspection and control for one queue row.
//!
//! Tracker URLs on private trackers carry a passkey, so nothing here ever returns one. The
//! API addresses an entry by the stable id derived from its URL and answers with the
//! redacted form only; a client that wants to remove or reorder a tracker sends ids.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::{
    DownloadId, TorrentTracker, TrackerOrigin, TrackerScrape, redact_tracker_url, tracker_id,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// One tracker as the API exposes it.
#[derive(Debug, Serialize, ToSchema)]
pub struct TrackerView {
    /// Stable id derived from the URL; safe to expose and used to address the entry.
    pub id: String,
    /// The announce URL with userinfo, passkey parameters and passkey path segments
    /// replaced. The full URL never leaves the service.
    pub url: String,
    pub tier: u16,
    pub origin: TrackerOrigin,
    pub last_announce_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Redaction-safe reason of the last failure.
    pub last_error: Option<String>,
    pub scrape: Option<TrackerScrape>,
    /// Whether the scrape counters are older than the freshness window. Stale counters are
    /// still returned, but never presented as current.
    pub scrape_stale: bool,
}

impl TrackerView {
    fn new(tracker: &TorrentTracker, now: chrono::DateTime<chrono::Utc>) -> Self {
        Self {
            id: tracker.id.clone(),
            url: redact_tracker_url(&tracker.url),
            tier: tracker.tier,
            origin: tracker.origin,
            last_announce_at: tracker.last_announce_at,
            last_error: tracker.last_error.clone(),
            scrape: tracker.scrape,
            scrape_stale: tracker
                .scrape
                .is_some_and(|scrape| rd_torrent::is_stale(scrape.scraped_at, now)),
        }
    }
}

/// The tracker list of one torrent.
#[derive(Debug, Serialize, ToSchema)]
pub struct TrackerListResponse {
    pub trackers: Vec<TrackerView>,
    /// Whether the engine can act on tracker edits at all.
    pub editable: bool,
}

/// Replaces the tracker list.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default)]
pub struct TrackerListRequest {
    /// Announce URLs in the order they should be tried. The list is authoritative: an
    /// entry missing from it is removed.
    pub trackers: Vec<TrackerEntryRequest>,
}

/// One tracker in a replacement request.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct TrackerEntryRequest {
    /// Id of an existing tracker to keep. Set this instead of `url` to keep an entry whose
    /// URL the client never sees in full.
    #[serde(default)]
    pub id: Option<String>,
    /// A new announce URL to add.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tier: u16,
}

/// Reads the tracker list of one torrent.
#[utoipa::path(
    get,
    path = "/api/v1/downloads/{id}/torrent/trackers",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = TrackerListResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn list_trackers(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<TrackerListResponse>, ApiError> {
    let stored = crate::torrent_control::require_download_state(&state, id).await?;
    Ok(Json(view(&state, stored.metadata.as_ref())))
}

/// Replaces the tracker list of one torrent.
#[utoipa::path(
    put,
    path = "/api/v1/downloads/{id}/torrent/trackers",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    request_body = TrackerListRequest,
    responses(
        (status = 200, body = TrackerListResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn put_trackers(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
    Json(request): Json<TrackerListRequest>,
) -> Result<Json<TrackerListResponse>, ApiError> {
    if !state.torrent.capabilities().tracker_edit {
        return Err(crate::torrent_control::unsupported("tracker_edit"));
    }
    require_safe_state(&state, id).await?;
    let mut stored = crate::torrent_control::require_download_state(&state, id).await?;
    let metadata = stored.metadata.as_mut().ok_or_else(|| {
        ApiError::bad_request(
            "torrent.metadata_pending",
            "The torrent metadata has not been resolved yet",
        )
    })?;
    if request.trackers.len() > rd_core::MAX_TORRENT_TRACKERS {
        return Err(ApiError::bad_request(
            "torrent.tracker_limit",
            format!(
                "At most {} trackers are allowed",
                rd_core::MAX_TORRENT_TRACKERS
            ),
        )
        .with_param("max", rd_core::MAX_TORRENT_TRACKERS.to_string()));
    }
    let existing = metadata.trackers.clone();
    let mut replacement: Vec<TorrentTracker> = Vec::with_capacity(request.trackers.len());
    for entry in request.trackers {
        let mut tracker = match (entry.id.as_deref(), entry.url.as_deref()) {
            // Keeping an existing entry preserves its full URL, which the client cannot see.
            (Some(id), _) => existing
                .iter()
                .find(|tracker| tracker.id == id)
                .cloned()
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "torrent.tracker_url_invalid",
                        "The tracker to keep does not exist",
                    )
                })?,
            (None, Some(url)) => {
                TorrentTracker::new(validate_tracker_url(url)?, entry.tier, TrackerOrigin::User)
            }
            (None, None) => {
                return Err(ApiError::bad_request(
                    "torrent.tracker_url_invalid",
                    "A tracker entry needs either an id or a url",
                ));
            }
        };
        tracker.tier = entry.tier;
        if !replacement.iter().any(|existing| existing.id == tracker.id) {
            replacement.push(tracker);
        }
    }
    metadata.trackers = replacement;
    state
        .database
        .set_download_torrent_state(id, stored.clone())
        .await?;
    Ok(Json(view(&state, stored.metadata.as_ref())))
}

/// Forces a fresh announce to every tracker.
#[utoipa::path(
    post,
    path = "/api/v1/downloads/{id}/torrent/trackers/reannounce",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = crate::dto::MessageResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 429, body = crate::error::ErrorBody)
    )
)]
pub async fn reannounce(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state.torrent.reannounce(id).await.map_err(|error| {
        let message = error.to_string();
        // The reason comes from rd-torrent as a value. Matching the word "wait" in its message
        // meant a reworded refusal quietly turned a documented 429 into a 400.
        match rd_torrent::torrent_kind(&error) {
            Some(rd_torrent::TorrentErrorKind::RateLimited) => {
                ApiError::too_many_requests("torrent.reannounce_rate_limited", message)
            }
            // An engine failure is not "not active" either, but that is the answer this route
            // has always given for anything it could not place, and narrowing it is a separate
            // decision from removing the string match.
            Some(rd_torrent::TorrentErrorKind::NotActive) | None => {
                ApiError::bad_request("torrent.not_active", message)
            }
        }
    })?;
    Ok(Json(crate::dto::MessageResponse::new(
        "torrent.reannounced",
        "Announcing to the trackers again",
    )))
}

/// Refreshes the scrape counters of every tracker.
#[utoipa::path(
    post,
    path = "/api/v1/downloads/{id}/torrent/trackers/scrape",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = TrackerListResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn scrape_trackers(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<TrackerListResponse>, ApiError> {
    if !state.torrent.capabilities().tracker_scrape {
        return Err(crate::torrent_control::unsupported("tracker_scrape"));
    }
    let stored =
        state.torrent.scrape_trackers(id).await.map_err(|error| {
            ApiError::bad_request("torrent.scrape_failed", format!("{error:#}"))
        })?;
    Ok(Json(view(&state, stored.metadata.as_ref())))
}

/// Builds the redacted list response.
fn view(state: &AppState, metadata: Option<&rd_core::TorrentMetadataInfo>) -> TrackerListResponse {
    let now = chrono::Utc::now();
    TrackerListResponse {
        trackers: metadata
            .map(|metadata| {
                metadata
                    .trackers
                    .iter()
                    .map(|tracker| TrackerView::new(tracker, now))
                    .collect()
            })
            .unwrap_or_default(),
        editable: state.torrent.capabilities().tracker_edit,
    }
}

/// Trackers may only be edited while the row is in a state where a re-add is safe.
async fn require_safe_state(state: &AppState, id: DownloadId) -> Result<(), ApiError> {
    let download = state
        .database
        .get_download(id)
        .await?
        .ok_or_else(crate::error_codes::download_not_found)?;
    if matches!(
        download.state,
        rd_core::DownloadState::Downloading
            | rd_core::DownloadState::Paused
            | rd_core::DownloadState::Queued
            | rd_core::DownloadState::Seeding
    ) {
        return Ok(());
    }
    Err(ApiError::conflict(
        "torrent.state_locked",
        "Trackers can only be edited while the torrent is queued, running, paused or seeding",
    ))
}

/// Accepts only an announce URL a tracker could actually be reached at.
fn validate_tracker_url(url: &str) -> Result<String, ApiError> {
    let invalid = || {
        ApiError::bad_request(
            "torrent.tracker_url_invalid",
            "A tracker URL must be http, https or udp",
        )
    };
    if url.len() > rd_core::MAX_TRACKER_URL {
        return Err(ApiError::bad_request(
            "torrent.tracker_url_invalid",
            "The tracker URL is too long",
        ));
    }
    let parsed = url::Url::parse(url).map_err(|_| invalid())?;
    if !matches!(parsed.scheme(), "http" | "https" | "udp") || parsed.host_str().is_none() {
        return Err(invalid());
    }
    // Derived here so a caller cannot smuggle in a mismatched id.
    let _ = tracker_id(url);
    Ok(url.to_owned())
}

/// Query parameters of the peer list.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct PeerPageQuery {
    /// Page size; clamped to the server maximum.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Opaque cursor from the previous page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// Aggregate counters of one torrent.
#[utoipa::path(
    get,
    path = "/api/v1/downloads/{id}/torrent/stats",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = rd_core::TorrentAggregateStats),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn torrent_stats(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<rd_core::TorrentAggregateStats>, ApiError> {
    Ok(Json(state.torrent.aggregate_stats(id).await.map_err(
        |error| ApiError::bad_request("torrent.stats_unavailable", format!("{error:#}")),
    )?))
}

/// One page of the peer list.
#[utoipa::path(
    get,
    path = "/api/v1/downloads/{id}/torrent/peers",
    tag = "downloads",
    params(("id" = DownloadId, Path), PeerPageQuery),
    responses(
        (status = 200, body = rd_core::TorrentPeerPage),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn torrent_peers(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
    axum::extract::Query(query): axum::extract::Query<PeerPageQuery>,
) -> Result<Json<rd_core::TorrentPeerPage>, ApiError> {
    if !state.torrent.capabilities().peer_stats {
        return Err(crate::torrent_control::unsupported("peer_stats"));
    }
    let limit = rd_torrent::peer_page_size(query.limit);
    Ok(Json(
        state
            .torrent
            .peer_page(id, limit, query.cursor)
            .await
            .map_err(|error| {
                ApiError::bad_request("torrent.stats_unavailable", format!("{error:#}"))
            })?,
    ))
}

/// Bucketed piece availability of one torrent.
#[utoipa::path(
    get,
    path = "/api/v1/downloads/{id}/torrent/pieces",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = rd_core::TorrentPieceAvailability),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn torrent_pieces(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<rd_core::TorrentPieceAvailability>, ApiError> {
    if !state.torrent.capabilities().piece_stats {
        return Err(crate::torrent_control::unsupported("piece_stats"));
    }
    Ok(Json(state.torrent.piece_availability(id).await.map_err(
        |error| ApiError::bad_request("torrent.stats_unavailable", format!("{error:#}")),
    )?))
}
