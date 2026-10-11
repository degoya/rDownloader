//! Torrent endpoints: seeding control, and the recheck and move of a torrent's data. The
//! `.torrent` import is the intake area's (`rd_api_intake::torrent_import`).

use axum::{
    Json,
    extract::{Path, State},
};

use crate::{
    ApiError, AppState, dto::MessageResponse, torrent_intake::ensure_torrent_service_enabled,
};

mod actions;

pub use actions::*;

/// What the embedded torrent engine supports.
///
/// The UI reads this once and disables the controls the engine cannot honour, instead of
/// offering switches that would be silently ignored.
#[utoipa::path(
    get,
    path = "/api/v1/torrents/capabilities",
    tag = "downloads",
    responses((status = 200, body = rd_core::TorrentEngineCapabilities))
)]
pub async fn torrent_capabilities(
    State(state): State<AppState>,
) -> Json<rd_core::TorrentEngineCapabilities> {
    Json(state.torrent.capabilities())
}

/// Network interfaces the torrent engine can bind to.
#[utoipa::path(
    get,
    path = "/api/v1/torrents/network/interfaces",
    tag = "downloads",
    responses((status = 200, body = Vec<rd_torrent::NetworkInterface>))
)]
pub async fn torrent_interfaces() -> Json<Vec<rd_torrent::NetworkInterface>> {
    Json(rd_torrent::interfaces())
}

/// What the torrent network layer is currently doing.
#[utoipa::path(
    get,
    path = "/api/v1/torrents/network/status",
    tag = "downloads",
    responses((status = 200, body = rd_torrent::TorrentNetworkStatus))
)]
pub async fn torrent_network_status(
    State(state): State<AppState>,
) -> Json<rd_torrent::TorrentNetworkStatus> {
    Json(state.torrent.network_status().await)
}

/// Tests the incoming peer port from this machine alone, no outside service asked
/// (RD-1240-16); starts the torrent engine when it is not running yet.
#[utoipa::path(
    post,
    path = "/api/v1/torrents/network/port-test",
    tag = "downloads",
    responses(
        (status = 200, body = rd_torrent::TorrentPortTest),
        (status = 400, description = "The BitTorrent service is switched off")
    )
)]
pub async fn test_torrent_port(
    State(state): State<AppState>,
) -> Result<Json<rd_torrent::TorrentPortTest>, ApiError> {
    ensure_torrent_service_enabled(&state).await?;
    Ok(Json(state.torrent.port_test().await))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/seeding/stop", tag = "downloads", params(("id" = rd_core::DownloadId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn stop_seeding(
    State(state): State<AppState>,
    Path(id): Path<rd_core::DownloadId>,
) -> Result<Json<MessageResponse>, ApiError> {
    if state.torrent.stop_seeding(id).await? {
        Ok(Json(MessageResponse::new(
            "torrent.seeding_stopped",
            "Seeding stopped; the download is complete",
        )))
    } else {
        Err(ApiError::not_found(
            "torrent.not_seeding",
            "This download is not seeding",
        ))
    }
}
