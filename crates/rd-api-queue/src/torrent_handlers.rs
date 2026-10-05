//! Torrent endpoints: `.torrent` intake, seeding control, and the recheck and move of a
//! torrent's data.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};

use crate::{
    ApiError, AppState,
    dto::{CollectorIntakeResponse, MessageResponse},
    torrent_intake::{add_torrent_to_collector, ensure_torrent_service_enabled},
};

mod actions;

pub use actions::*;

#[utoipa::path(post, path = "/api/v1/torrents/import", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (crate::container_upload::ContainerUpload = "application/json"))), responses((status = 201, body = CollectorIntakeResponse), (status = 400, description = "The torrent is invalid or over 16 MiB, the service is off, a field is invalid, or the JSON content is not base64"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit")))]
pub async fn import_torrent(
    State(state): State<AppState>,
    body: crate::container_upload::UploadBody,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    // A `.torrent` upload does not go through the LinkGrabber's intake, so the switch has to
    // be honoured here as well; otherwise the one path that bypasses it stays open.
    ensure_torrent_service_enabled(&state).await?;
    let upload = body.read().await?;
    let category_id = match upload.category_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(id) => Some(id.parse::<rd_core::CategoryId>().map_err(|_| {
            ApiError::bad_request("torrent.category_invalid", "Category id is not valid")
        })?),
    };
    let priority = match upload.priority.as_deref().map(str::trim) {
        None => None,
        Some("low") => Some(rd_core::DownloadPriority::Low),
        Some("normal") => Some(rd_core::DownloadPriority::Normal),
        Some("high") => Some(rd_core::DownloadPriority::High),
        Some(_) => {
            return Err(ApiError::bad_request(
                "torrent.priority_invalid",
                "Torrent priority is not valid",
            ));
        }
    };
    let package_name = upload.name;
    let Some(file) = upload.file else {
        return Err(ApiError::bad_request(
            "request.multipart_missing_file",
            "Multipart field 'file' is missing",
        ));
    };
    if file.bytes.len() > rd_torrent::MAX_TORRENT_BYTES {
        return Err(ApiError::bad_request(
            "torrent.file_too_large",
            "Torrent file exceeds the 16 MiB limit",
        ));
    }
    let source_label = file.file_name;
    let content = file.bytes;
    rd_torrent::parse_torrent(&content)
        .map_err(|error| ApiError::bad_request("torrent.file_invalid", format!("{error:#}")))?;
    let (batch, packages, candidates) = add_torrent_to_collector(
        &state.database,
        &state.torrent,
        &content,
        rd_core::IngressSource::Manual,
        source_label,
        package_name,
        category_id,
        priority,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CollectorIntakeResponse {
            batch,
            packages,
            candidates,
            skipped_excluded: 0,
            skipped_disabled: 0,
            // A torrent is handed over as a file or a magnet; no crawler is asked.
            crawled_found: 0,
            crawled_dropped: 0,
        }),
    ))
}

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
