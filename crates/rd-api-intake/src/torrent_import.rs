//! `POST /api/v1/torrents/import`: a `.torrent` into the LinkGrabber, or straight on into the
//! download list.
//!
//! The route lived in the queue area, which may not call the LinkGrabber's enqueue, so the
//! `enqueue` field its shared body documents was read by nobody (RD-1240-28). It belongs here,
//! with the other imports: a torrent's link needs no online check, so `enqueue` queues its
//! package at once, through the same enqueue a click in the LinkGrabber runs.

use axum::{Json, extract::State, http::StatusCode};

use crate::{
    ApiError, AppState,
    dto::CollectorIntakeResponse,
    torrent_intake::{add_torrent_to_collector, ensure_torrent_service_enabled},
};

#[utoipa::path(post, path = "/api/v1/torrents/import", tag = "collector", request_body(content((Vec<u8> = "multipart/form-data"), (crate::container_upload::ContainerUpload = "application/json"))), responses((status = 201, body = CollectorIntakeResponse), (status = 400, description = "The torrent is invalid or over 16 MiB, the service is off, a field is invalid, or the JSON content is not base64"), (status = 409, description = "`enqueue` was set and the package could not be queued; it stays in the LinkGrabber"), (status = 413, description = "The JSON content decodes to more than 48 MiB, or the body exceeds the service's limit")))]
pub async fn import_torrent(
    State(state): State<AppState>,
    body: crate::container_upload::UploadBody,
) -> Result<(StatusCode, Json<CollectorIntakeResponse>), ApiError> {
    // A `.torrent` upload does not go through the LinkGrabber's intake, so the switch has to
    // be honoured here as well; otherwise the one path that bypasses it stays open.
    ensure_torrent_service_enabled(&state).await?;
    let upload = body.read().await?;
    let enqueue = crate::container_handlers::enqueue_flag(upload.enqueue.as_deref())?;
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
    if enqueue {
        for package in &packages {
            crate::collector_enqueue::enqueue_package(&state, package.id, false, None).await?;
        }
    }
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
