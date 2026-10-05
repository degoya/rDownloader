//! What the queue amounts to: the summary, and the moving transfer rates.

use super::*;

#[utoipa::path(get, path = "/api/v1/downloads/summary", tag = "downloads", responses((status = 200, body = DownloadSummaryResponse)))]
pub async fn download_summary(
    State(state): State<AppState>,
) -> Result<Json<DownloadSummaryResponse>, ApiError> {
    Ok(Json(summarize_downloads(&state).await?))
}

pub async fn summarize_downloads(state: &AppState) -> Result<DownloadSummaryResponse, ApiError> {
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
    let queue_rate = queue_rate(
        &moving_rates(state.scheduler.transfer_rates(), &downloads),
        &downloads,
    );
    let mut storage = Vec::new();
    for root in state.database.list_storage_roots().await? {
        let path = root.path.clone();
        let space = tokio::task::spawn_blocking(move || {
            let free = fs4::available_space(&path).ok();
            let total = fs4::total_space(&path).ok();
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

pub(super) fn byte_count(value: u64) -> rd_core::ByteCount {
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
pub(super) fn remaining_of(download: &rd_core::DownloadFile) -> Option<u64> {
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

/// The rates of the downloads that are moving bytes right now.
///
/// The scheduler's rates are smoothed and sampled every other tick, so a transfer that just
/// finished still carries its last rate for a few seconds. Without this filter the page, which
/// reads the rates once more when the finish is announced and then hears nothing further, kept
/// showing that last speed on a finished package until it was reloaded (1.5, Premiumize remote
/// job).
pub(crate) fn moving_rates(
    rates: std::collections::HashMap<rd_core::DownloadId, u64>,
    downloads: &[rd_core::DownloadFile],
) -> std::collections::HashMap<rd_core::DownloadId, u64> {
    let moving = downloads
        .iter()
        .filter(|download| download.state == rd_core::DownloadState::Downloading)
        .map(|download| download.id)
        .collect::<std::collections::HashSet<_>>();
    rates
        .into_iter()
        .filter(|(id, _)| moving.contains(id))
        .collect()
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
    let rates = moving_rates(state.scheduler.transfer_rates(), &downloads);
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
