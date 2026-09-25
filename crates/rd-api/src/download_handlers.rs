use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_core::DownloadId;
use rd_db::StoreErrorKind;
use url::Url;

use crate::{
    ApiError, AppState,
    dto::{
        CreateDownloadRequest, DownloadBulkAction, DownloadBulkRequest, DownloadBulkResponse,
        DownloadExtractRequest, DownloadRateEntry, DownloadRatesResponse, DownloadRenameRequest,
        DownloadSummaryResponse, MessageResponse, StorageSpace,
    },
    error_codes::parse_id,
};

#[utoipa::path(get, path = "/api/v1/downloads", tag = "downloads", responses((status = 200, body = [rd_core::DownloadFile])))]
pub async fn list_downloads(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::DownloadFile>>, ApiError> {
    Ok(Json(state.database.list_downloads().await?))
}

#[utoipa::path(get, path = "/api/v1/downloads/summary", tag = "downloads", responses((status = 200, body = DownloadSummaryResponse)))]
pub async fn download_summary(
    State(state): State<AppState>,
) -> Result<Json<DownloadSummaryResponse>, ApiError> {
    Ok(Json(summarize_downloads(&state).await?))
}

pub(crate) async fn summarize_downloads(
    state: &AppState,
) -> Result<DownloadSummaryResponse, ApiError> {
    use rd_core::DownloadState;
    let downloads = state.database.list_downloads().await?;
    let count = |predicate: fn(DownloadState) -> bool| {
        u32::try_from(
            downloads
                .iter()
                .filter(|download| predicate(download.state))
                .count(),
        )
        .unwrap_or(u32::MAX)
    };
    let total_bytes: u64 = downloads
        .iter()
        .filter_map(|download| download.total_bytes.map(rd_core::ByteCount::get))
        .fold(0, u64::saturating_add);
    let committed_bytes: u64 = downloads
        .iter()
        .map(|download| download.committed_bytes.get())
        .fold(0, u64::saturating_add);
    let remaining_bytes: u64 = downloads
        .iter()
        .filter(|download| {
            !matches!(
                download.state,
                DownloadState::Completed
                    | DownloadState::Cancelled
                    | DownloadState::Failed
                    // A mirror that is not going to download has no bytes left to fetch;
                    // counting them would inflate the remainder by every alternative link.
                    | DownloadState::Skipped
            )
        })
        .filter_map(|download| {
            download
                .total_bytes
                .map(|total| total.get().saturating_sub(download.committed_bytes.get()))
        })
        .fold(0, u64::saturating_add);
    let queue_rate = queue_rate(&state.scheduler.transfer_rates(), &downloads);
    let mut storage = Vec::new();
    for root in state.database.list_storage_roots().await? {
        let path = root.path.clone();
        let space = tokio::task::spawn_blocking(move || {
            let free = fs2::available_space(&path).ok();
            let total = fs2::total_space(&path).ok();
            (free, total)
        })
        .await
        .map_err(|error| anyhow::anyhow!("storage probe task failed: {error}"))?;
        storage.push(StorageSpace {
            id: root.id,
            name: root.name,
            path: root.path,
            is_default: root.is_default,
            free_bytes: space
                .0
                .and_then(|value| rd_core::ByteCount::new(value).ok()),
            total_bytes: space
                .1
                .and_then(|value| rd_core::ByteCount::new(value).ok()),
        });
    }
    Ok(DownloadSummaryResponse {
        queued: count(|state| matches!(state, DownloadState::Queued | DownloadState::RetryWait)),
        active: count(|state| {
            matches!(
                state,
                DownloadState::Resolving
                    | DownloadState::Downloading
                    | DownloadState::Verifying
                    | DownloadState::Repairing
                    | DownloadState::Extracting
            )
        }),
        paused: count(|state| state == DownloadState::Paused),
        blocked: count(|state| state == DownloadState::Blocked),
        failed: count(|state| matches!(state, DownloadState::Failed | DownloadState::Cancelled)),
        completed: count(|state| state == DownloadState::Completed),
        total_bytes: byte_count(total_bytes),
        committed_bytes: byte_count(committed_bytes),
        remaining_bytes: byte_count(remaining_bytes),
        transferring_remaining_bytes: queue_rate.remaining_bytes.map(byte_count),
        bytes_per_second: queue_rate.bytes_per_second,
        eta_seconds: queue_rate.eta_seconds,
        storage,
    })
}

fn byte_count(value: u64) -> rd_core::ByteCount {
    rd_core::ByteCount::new(value).unwrap_or_default()
}

/// States whose bytes are still going to come down the wire.
///
/// This is the line the remaining time is drawn along, and it is deliberately narrower than
/// the one `remaining_bytes` uses. A paused or blocked entry is not moving and will not move
/// without somebody saying so; an entry being verified, repaired, unpacked or seeded has
/// already been fetched. Counting either into "how long until the queue is through at the
/// current speed" would answer a different question than the one asked.
pub(crate) fn is_transferring(state: rd_core::DownloadState) -> bool {
    use rd_core::DownloadState;
    matches!(
        state,
        DownloadState::Queued
            | DownloadState::RetryWait
            | DownloadState::Resolving
            | DownloadState::Downloading
    )
}

/// What one entry still has to fetch, or `None` when its size is not known.
fn remaining_of(download: &rd_core::DownloadFile) -> Option<u64> {
    download
        .total_bytes
        .map(|total| total.get().saturating_sub(download.committed_bytes.get()))
}

/// The queue's combined rate and the remaining time it implies.
pub(crate) struct QueueRate {
    pub bytes_per_second: u64,
    /// `None` while any entry still to be fetched has no known size — the sum would be a lower
    /// bound, and presenting a lower bound as an estimate is the invented number this job
    /// exists to avoid.
    pub remaining_bytes: Option<u64>,
    pub eta_seconds: Option<u64>,
}

pub(crate) fn queue_rate(
    rates: &std::collections::HashMap<rd_core::DownloadId, u64>,
    downloads: &[rd_core::DownloadFile],
) -> QueueRate {
    let bytes_per_second = downloads
        .iter()
        .filter_map(|download| rates.get(&download.id).copied())
        .fold(0u64, u64::saturating_add);
    let mut remaining_bytes = Some(0u64);
    for download in downloads.iter().filter(|item| is_transferring(item.state)) {
        remaining_bytes = match (remaining_bytes, remaining_of(download)) {
            (Some(sum), Some(remaining)) => Some(sum.saturating_add(remaining)),
            _ => None,
        };
    }
    QueueRate {
        bytes_per_second,
        remaining_bytes,
        eta_seconds: rd_scheduler::estimate_seconds(remaining_bytes, bytes_per_second),
    }
}

#[utoipa::path(get, path = "/api/v1/downloads/rates", tag = "downloads", responses((status = 200, body = DownloadRatesResponse)))]
pub async fn download_rates(
    State(state): State<AppState>,
) -> Result<Json<DownloadRatesResponse>, ApiError> {
    let downloads = state.database.list_downloads().await?;
    let rates = state.scheduler.transfer_rates();
    let queue = queue_rate(&rates, &downloads);
    let entries = downloads
        .iter()
        .filter_map(|download| {
            let bytes_per_second = rates.get(&download.id).copied().unwrap_or_default();
            // Only what is moving. A rate of zero carries no estimate either, so a row for it
            // would say nothing the absence does not already say.
            (bytes_per_second > 0).then(|| DownloadRateEntry {
                id: download.id,
                bytes_per_second,
                eta_seconds: rd_scheduler::estimate_seconds(
                    remaining_of(download),
                    bytes_per_second,
                ),
            })
        })
        .collect();
    Ok(Json(DownloadRatesResponse {
        bytes_per_second: queue.bytes_per_second,
        transferring_remaining_bytes: queue.remaining_bytes.map(byte_count),
        eta_seconds: queue.eta_seconds,
        downloads: entries,
    }))
}

#[utoipa::path(post, path = "/api/v1/downloads", tag = "downloads", request_body = CreateDownloadRequest, responses((status = 201, body = rd_core::DownloadFile)))]
pub async fn create_download(
    State(state): State<AppState>,
    Json(request): Json<CreateDownloadRequest>,
) -> Result<(StatusCode, Json<rd_core::DownloadFile>), ApiError> {
    let file = create_download_inner(&state, request).await?;
    Ok((StatusCode::CREATED, Json(file)))
}

pub(crate) async fn create_download_inner(
    state: &AppState,
    request: CreateDownloadRequest,
) -> Result<rd_core::DownloadFile, ApiError> {
    let url = Url::parse(&request.url)
        .map(rd_collector::canonical_url)
        .map_err(|error| ApiError::bad_request("download.url_invalid", error.to_string()))?;
    if url.scheme() == "magnet" {
        let name = request
            .package_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| crate::torrent_handlers::magnet_name(&url));
        let package = crate::torrent_handlers::enqueue_torrent(
            state,
            url,
            name,
            None,
            request.category_id,
            request.priority.unwrap_or_default(),
        )
        .await?;
        let file = state
            .database
            .list_downloads()
            .await?
            .into_iter()
            .find(|file| file.package_id == package.id)
            .ok_or_else(|| anyhow::anyhow!("torrent download row missing"))?;
        return Ok(file);
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ApiError::bad_request(
            "download.url_scheme_unsupported",
            "Only HTTP(S) or magnet URLs are supported for direct downloads",
        ));
    }
    let inferred = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|value| !value.is_empty())
        .unwrap_or("download.bin")
        .to_owned();
    // A URL-derived package name doubles as the folder name; extensions are stripped.
    let package_name = request
        .package_name
        .unwrap_or_else(|| rd_files::package_name_from_file_name(&inferred));
    let file_name = request.file_name.unwrap_or(inferred);
    validate_network_selection(state, request.account_id, request.proxy_profile_id).await?;
    let account_id = match request.account_id {
        Some(id) => Some(id),
        None => crate::hosters::fallback_account(state, &url).await,
    };
    let destination =
        crate::config_handlers::download_destination(state, request.category_id).await?;
    let options = rd_scheduler::PackageOptions {
        category_id: request.category_id,
        priority: request.priority.unwrap_or_default(),
    };
    let file = if let Some(destination) = destination {
        state
            .scheduler
            .enqueue_direct_to_with_network(
                url,
                package_name,
                file_name,
                destination,
                account_id,
                request.proxy_profile_id,
                options,
            )
            .await?
    } else {
        state
            .scheduler
            .enqueue_direct_with_network(
                url,
                package_name,
                file_name,
                account_id,
                request.proxy_profile_id,
                options,
            )
            .await?
    };
    Ok(file)
}

async fn validate_network_selection(
    state: &AppState,
    account_id: Option<rd_core::AccountId>,
    proxy_id: Option<rd_core::ProxyProfileId>,
) -> Result<(), ApiError> {
    if let Some(account_id) = account_id {
        let account = state
            .database
            .list_accounts()
            .await?
            .into_iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| {
                ApiError::bad_request("account.not_found", "Provider account not found")
            })?;
        if !account.enabled {
            return Err(ApiError::bad_request(
                "account.disabled",
                "Provider account is disabled",
            ));
        }
    }
    if let Some(proxy_id) = proxy_id
        && !state
            .database
            .list_proxy_profiles()
            .await?
            .iter()
            .any(|profile| profile.id == proxy_id)
    {
        return Err(ApiError::bad_request(
            "proxy.not_found",
            "Proxy profile not found",
        ));
    }
    Ok(())
}

#[utoipa::path(patch, path = "/api/v1/downloads/{id}", tag = "downloads", params(("id" = String, Path)), request_body = DownloadRenameRequest, responses((status = 200, body = rd_core::DownloadFile), (status = 404), (status = 409)))]
pub async fn rename_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<DownloadRenameRequest>,
) -> Result<Json<rd_core::DownloadFile>, ApiError> {
    let file_name = rd_files::sanitize_file_name(request.file_name.trim());
    if file_name.is_empty() || file_name.chars().count() > 255 {
        return Err(ApiError::bad_request(
            "download.file_name_length",
            "File name must be between 1 and 255 characters",
        )
        .with_param("max", 255));
    }
    state
        .database
        .rename_download(parse_id::<DownloadId>(&id)?, file_name)
        .await
        .map(Json)
        .map_err(|error| match rd_db::store_kind(&error) {
            Some(StoreErrorKind::WrongState) => ApiError::conflict(
                "download.rename_state",
                "Only queued, paused or failed downloads can be renamed",
            ),
            Some(StoreErrorKind::NotFound) => crate::error_codes::download_not_found(),
            _ => ApiError::from(error),
        })
}

#[utoipa::path(post, path = "/api/v1/downloads/bulk", tag = "downloads", request_body = DownloadBulkRequest, responses((status = 200, body = DownloadBulkResponse)))]
pub async fn bulk_downloads(
    State(state): State<AppState>,
    Json(request): Json<DownloadBulkRequest>,
) -> Result<Json<DownloadBulkResponse>, ApiError> {
    Ok(Json(
        apply_download_action(&state, request.action, request.ids).await?,
    ))
}

pub(crate) async fn apply_download_action(
    state: &AppState,
    action: DownloadBulkAction,
    ids: Vec<DownloadId>,
) -> Result<DownloadBulkResponse, ApiError> {
    if ids.is_empty() || ids.len() > 500 {
        return Err(crate::error_codes::bulk_range(500));
    }
    let mut affected = 0_u32;
    let mut errors = Vec::new();
    for id in ids {
        let result = match action {
            DownloadBulkAction::Pause => state.scheduler.pause(id).await,
            DownloadBulkAction::Resume => state.scheduler.resume(id).await,
            DownloadBulkAction::Cancel => state.scheduler.cancel(id).await,
            DownloadBulkAction::Remove => remove_with_cancel(state, id).await,
            DownloadBulkAction::Reset => reset_download_file(state, id, false).await,
            DownloadBulkAction::ResetDeleteFiles => reset_download_file(state, id, true).await,
        };
        match result {
            Ok(()) => affected += 1,
            Err(error) => errors.push(format!("{id}: {error}")),
        }
    }
    Ok(DownloadBulkResponse { affected, errors })
}

/// Discards a file's data and queues it again from zero.
///
/// A torrent additionally leaves the persisted librqbit session, because a handle that still
/// knows the old pieces would resume them instead of fetching the data again.
pub(crate) async fn reset_download_file(
    state: &AppState,
    id: DownloadId,
    delete_completed_files: bool,
) -> anyhow::Result<()> {
    state.scheduler.reset(id, delete_completed_files).await?;
    state.torrent.forget(id).await;
    Ok(())
}

/// Removes one queue row, and with it the row's torrent from the engine session.
///
/// The one removal path: the single `DELETE`, the bulk action, package deletion — and through
/// that auto-remove, the SABnzbd and qBittorrent compatibility APIs and MCP — all end here.
/// Only the single `DELETE` used to forget the torrent, so a torrent removed any other way
/// stayed in the persisted librqbit session and had its files created again on every start
/// (RD-120-68). The torrent is located first because its info hash is stored on the row.
/// The payload stays on disk, as for every other kind of download.
pub(crate) async fn remove_download(state: &AppState, id: DownloadId) -> anyhow::Result<()> {
    let torrent = state.torrent.locate(id).await;
    state.scheduler.remove(id).await?;
    state.torrent.forget_located(torrent).await;
    Ok(())
}

/// Cancels an active file first and waits briefly for its token to clear before removal.
pub(crate) async fn remove_with_cancel(state: &AppState, id: DownloadId) -> anyhow::Result<()> {
    if let Some(current) = state.database.get_download(id).await?
        && matches!(
            current.state,
            rd_core::DownloadState::Resolving
                | rd_core::DownloadState::Downloading
                | rd_core::DownloadState::Verifying
                | rd_core::DownloadState::Repairing
                | rd_core::DownloadState::Extracting
        )
    {
        state.scheduler.cancel(id).await?;
        for _ in 0..25 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if remove_download(state, id).await.is_ok() {
                return Ok(());
            }
        }
    }
    remove_download(state, id).await
}

#[utoipa::path(post, path = "/api/v1/downloads/extract", tag = "downloads", request_body = DownloadExtractRequest, responses((status = 202, body = MessageResponse), (status = 409)))]
pub async fn extract_downloads(
    State(state): State<AppState>,
    Json(request): Json<DownloadExtractRequest>,
) -> Result<(StatusCode, Json<MessageResponse>), ApiError> {
    if request.ids.is_empty() || request.ids.len() > 500 {
        return Err(crate::error_codes::bulk_range(500));
    }
    let downloads = state.database.list_downloads().await?;
    let mut packages: Vec<rd_core::PackageId> = downloads
        .iter()
        .filter(|file| {
            request.ids.contains(&file.id) && file.state == rd_core::DownloadState::Completed
        })
        .map(|file| file.package_id)
        .collect();
    packages.sort();
    packages.dedup();
    if packages.is_empty() {
        return Err(ApiError::conflict(
            "download.none_completed",
            "None of the selected files is completed",
        ));
    }
    let count = packages.len();
    for package_id in packages {
        state
            .extraction
            .request(package_id, rd_extract::ExtractionTrigger::Manual)
            .await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(
            MessageResponse::new(
                "package.extract_queued",
                format!("Extraction queued for {count} package(s)"),
            )
            .with_count(count),
        ),
    ))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/pause", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse)))]
pub async fn pause_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    state.scheduler.pause(parse_id::<DownloadId>(&id)?).await?;
    Ok(message("download.paused", "Download paused"))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/resume", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse)))]
pub async fn resume_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    state.scheduler.resume(parse_id::<DownloadId>(&id)?).await?;
    Ok(message("download.resumed", "Download resumed"))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/cancel", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse)))]
pub async fn cancel_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    state.scheduler.cancel(parse_id::<DownloadId>(&id)?).await?;
    Ok(message("download.cancelled", "Download cancelled"))
}

#[utoipa::path(post, path = "/api/v1/downloads/{id}/reset", tag = "downloads", params(("id" = String, Path)), request_body = crate::dto::DownloadResetRequest, responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn reset_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<crate::dto::DownloadResetRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let id = parse_id::<DownloadId>(&id)?;
    reset_download_file(&state, id, request.delete_completed_files)
        .await
        .map_err(reset_failure)?;
    Ok(message(
        "download.reset",
        "Download reset and queued again from the start",
    ))
}

/// Recognises a refusal from either layer of the reset and remove paths.
///
/// `rd-scheduler` checks the same two conditions before the store is ever reached, so it tags
/// its own refusals with the same reason `rd-db` uses. Neither layer's wording is read here,
/// which is the whole point: a reworded bail in either crate used to turn these documented
/// `409`s into `500`s with nothing to catch it.
fn refused(error: &anyhow::Error, kind: StoreErrorKind) -> bool {
    rd_db::store_kind(error) == Some(kind)
}

/// Maps the refusals of the reset path onto the codes the UI translates.
fn reset_failure(error: anyhow::Error) -> ApiError {
    if refused(&error, StoreErrorKind::WrongState) {
        ApiError::conflict(
            "download.active_must_pause",
            "Active downloads must be cancelled or paused before they can be reset",
        )
    } else if refused(&error, StoreErrorKind::NotFound) {
        crate::error_codes::download_not_found()
    } else {
        error.into()
    }
}

#[utoipa::path(delete, path = "/api/v1/downloads/{id}", tag = "downloads", params(("id" = String, Path)), responses((status = 200, body = MessageResponse), (status = 409)))]
pub async fn delete_download(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    let id = parse_id::<DownloadId>(&id)?;
    remove_download(&state, id).await.map_err(|error| {
        if refused(&error, StoreErrorKind::WrongState) {
            ApiError::conflict(
                "download.active_must_pause",
                "Active downloads must be cancelled or paused before removal",
            )
        } else if refused(&error, StoreErrorKind::NotFound) {
            crate::error_codes::download_not_found()
        } else {
            error.into()
        }
    })?;
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::DownloadDeleted)
            .by(&audit)
            .target("download", id),
    )
    .await;
    Ok(message(
        "download.removed",
        "Download removed from the download list",
    ))
}

fn message(code: &str, value: &str) -> Json<MessageResponse> {
    Json(MessageResponse::new(code, value))
}

#[utoipa::path(put, path = "/api/v1/downloads/{id}/auth-profile", tag = "downloads", params(("id" = rd_core::DownloadId, Path)), request_body = crate::dto::SetDownloadAuthProfileRequest, responses((status = 200, body = rd_core::DownloadFile), (status = 404)))]
pub async fn set_download_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<rd_core::DownloadId>,
    Json(request): Json<crate::dto::SetDownloadAuthProfileRequest>,
) -> Result<Json<rd_core::DownloadFile>, ApiError> {
    // A pinned profile must exist; silently storing a dangling id would only surface as a
    // failure at the next download attempt.
    if let rd_core::AuthProfileSelection::Pinned(profile_id) = request.auth_profile
        && state.database.auth_profile(profile_id).await?.is_none()
    {
        return Err(ApiError::not_found(
            "authprofile.not_found",
            "Auth profile not found",
        ));
    }
    state
        .database
        .set_download_auth_profile(id, request.auth_profile)
        .await
        .map_err(|_| crate::error_codes::download_not_found())?;
    state
        .database
        .get_download(id)
        .await?
        .map(Json)
        .ok_or_else(crate::error_codes::download_not_found)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{is_transferring, queue_rate, remaining_of};
    use rd_core::{DownloadFile, DownloadId, DownloadState};
    use std::collections::HashMap;

    /// Shared with `handlers`, which builds the capture summary out of the same queue.
    pub(crate) fn file(state: DownloadState, committed: u64, total: Option<u64>) -> DownloadFile {
        let now = chrono::Utc::now();
        DownloadFile {
            recording: None,
            id: DownloadId::new(),
            package_id: rd_core::PackageId::new(),
            source: "https://example.test/a.bin".parse().expect("url"),
            file_name: "a.bin".to_owned(),
            state,
            total_bytes: total.map(|value| rd_core::ByteCount::new(value).expect("size")),
            committed_bytes: rd_core::ByteCount::new(committed).unwrap_or_default(),
            retry_count: 0,
            next_retry_at: None,
            expected_checksum: None,
            computed_checksum: None,
            last_error: None,
            account_id: None,
            proxy_profile_id: None,
            remote_credential_id: None,
            mirror_group: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            position: 0,
            kind: rd_core::DownloadKind::Http,
            nzb_file_id: None,
            recovery: false,
            media: None,
            enrichment: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    pub(crate) fn rates(entries: &[(&DownloadFile, u64)]) -> HashMap<DownloadId, u64> {
        entries
            .iter()
            .map(|(file, rate)| (file.id, *rate))
            .collect()
    }

    #[test]
    fn the_queue_estimate_is_the_outstanding_bytes_over_the_combined_rate() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let waiting = file(DownloadState::Queued, 0, Some(1_000));
        let table = rates(&[(&running, 250)]);

        let queue = queue_rate(&table, &[running.clone(), waiting.clone()]);

        assert_eq!(queue.bytes_per_second, 250);
        assert_eq!(queue.remaining_bytes, Some(1_500));
        assert_eq!(queue.eta_seconds, Some(6));
    }

    /// Verifying, repairing, extracting and seeding have already been fetched; paused and
    /// blocked are not going anywhere. None of them belongs in "how long at the current speed".
    #[test]
    fn states_that_are_not_being_fetched_stay_out_of_the_estimate() {
        for state in [
            DownloadState::Paused,
            DownloadState::Blocked,
            DownloadState::Verifying,
            DownloadState::Repairing,
            DownloadState::Extracting,
            DownloadState::Seeding,
        ] {
            assert!(!is_transferring(state), "{state:?} must not be counted");
        }
        let running = file(DownloadState::Downloading, 0, Some(1_000));
        let paused = file(DownloadState::Paused, 0, Some(9_000_000));
        let table = rates(&[(&running, 100)]);

        let queue = queue_rate(&table, &[running.clone(), paused]);

        assert_eq!(queue.remaining_bytes, Some(1_000));
        assert_eq!(queue.eta_seconds, Some(10));
    }

    /// One entry of unknown size would make the sum a lower bound, and a lower bound presented
    /// as an estimate is exactly the invented number this must not produce.
    #[test]
    fn an_unknown_size_leaves_the_queue_without_an_estimate() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let sizeless = file(DownloadState::Queued, 0, None);
        let table = rates(&[(&running, 250)]);

        let queue = queue_rate(&table, &[running.clone(), sizeless]);

        assert_eq!(queue.remaining_bytes, None);
        assert_eq!(queue.eta_seconds, None);
    }

    #[test]
    fn a_still_queue_has_no_estimate_even_though_bytes_are_outstanding() {
        let paused = file(DownloadState::Paused, 100, Some(1_000));
        let waiting = file(DownloadState::Queued, 0, Some(1_000));

        let queue = queue_rate(&HashMap::new(), &[paused, waiting]);

        assert_eq!(queue.bytes_per_second, 0);
        assert_eq!(queue.remaining_bytes, Some(1_000));
        assert_eq!(queue.eta_seconds, None);
    }

    /// A checkpoint can overshoot the recorded total; the remainder floors at zero rather than
    /// wrapping into a very large number.
    #[test]
    fn an_overshooting_checkpoint_leaves_nothing_remaining() {
        let running = file(DownloadState::Downloading, 1_200, Some(1_000));
        assert_eq!(remaining_of(&running), Some(0));
    }
}
