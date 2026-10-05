//! The capture agent's figures: how much is running, queued and failed, and how fast.

use axum::{Extension, Json, extract::State};
use rd_api_core::auth::Granted;

use crate::{ApiError, AppState};

/// Figures the tray needs to say whether anything is running.
///
/// Same reasoning as the capture event stream (`event_stream::capture_events`): a capture token
/// is a narrow credential, so this answers with counts and byte totals and nothing that names a
/// file, a folder or an account.
#[utoipa::path(get, path = "/api/v1/capture/summary", tag = "capture", responses((status = 200, body = crate::dto::CaptureSummaryResponse)))]
pub async fn capture_summary(
    State(state): State<AppState>,
    granted: Option<Extension<Granted>>,
) -> Result<Json<crate::dto::CaptureSummaryResponse>, ApiError> {
    let downloads = state.database.list_downloads().await?;
    let mut figures = capture_figures(
        &downloads,
        &crate::download_handlers::moving_rates(state.scheduler.transfer_rates(), &downloads),
    );
    figures.paused_until = state.scheduler.queue_pause().await.map(|pause| pause.until);
    // Read from the grant `require_capture` resolved for this very request, so the tray learns
    // it may pause from the same lookup that will let it, and loses the entries with the right.
    figures.queue_control =
        granted.is_some_and(|Extension(granted)| granted.holds(rd_core::Scope::CaptureQueue));
    Ok(Json(figures))
}

/// Everything the capture summary says, once the queue has been read.
///
/// Split off from the handler so the counting boundary and the remaining time can be tested
/// without standing up a scheduler. The estimate is read out of the same `queue_rate()` value
/// the rate comes from; there is deliberately no second call and no second formula here.
pub(crate) fn capture_figures(
    downloads: &[rd_core::DownloadFile],
    rates: &std::collections::HashMap<rd_core::DownloadId, u64>,
) -> crate::dto::CaptureSummaryResponse {
    use rd_core::DownloadState;
    let count = |predicate: fn(DownloadState) -> bool| {
        u32::try_from(
            downloads
                .iter()
                .filter(|download| predicate(download.state))
                .count(),
        )
        .unwrap_or(u32::MAX)
    };
    let unfinished = |download: &rd_core::DownloadFile| {
        !matches!(
            download.state,
            DownloadState::Completed
                | DownloadState::Cancelled
                | DownloadState::Failed
                | DownloadState::Skipped
        )
    };
    let committed_bytes = downloads
        .iter()
        .filter(|download| unfinished(download))
        .map(|download| download.committed_bytes.get())
        .fold(0u64, u64::saturating_add);
    let total_bytes = downloads
        .iter()
        .filter(|download| unfinished(download))
        .filter_map(|download| download.total_bytes.map(rd_core::ByteCount::get))
        .fold(0u64, u64::saturating_add);
    // One reading of the queue's speed, and both figures it yields. `active` and `queued`
    // count `Downloading` and `Queued` alone, while the estimate follows `is_transferring`
    // and so also covers an entry resolving or waiting on a retry: the remaining time speaks
    // for the whole queue, the counts for two of its states (RD-108-01).
    let queue = crate::download_handlers::queue_rate(rates, downloads);
    crate::dto::CaptureSummaryResponse {
        active: count(|state| matches!(state, DownloadState::Downloading)),
        queued: count(|state| matches!(state, DownloadState::Queued)),
        failed: count(|state| matches!(state, DownloadState::Failed)),
        paused: count(|state| matches!(state, DownloadState::Paused)),
        committed_bytes: rd_core::ByteCount::new(committed_bytes).unwrap_or_default(),
        total_bytes: rd_core::ByteCount::new(total_bytes).unwrap_or_default(),
        // Counts, like everything else here: they say how much and how fast, never what.
        bytes_per_second: queue.bytes_per_second,
        eta_seconds: queue.eta_seconds,
        // The two the handler knows and this function does not: the queue's timed pause and
        // what the asking token holds.
        paused_until: None,
        queue_control: false,
    }
}

#[cfg(test)]
mod capture_summary_tests {
    use super::capture_figures;
    use crate::download_handlers::tests::{file, rates};
    use rd_core::DownloadState;

    /// The tray used to be handed the rate out of `queue_rate` while the estimate sitting in
    /// the same value was dropped. Both travel now, and both come from that one call.
    #[test]
    fn the_capture_summary_carries_the_queue_estimate_beside_the_rate() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let waiting = file(DownloadState::Queued, 0, Some(1_000));
        let table = rates(&[(&running, 250)]);

        let figures = capture_figures(&[running.clone(), waiting], &table);

        assert_eq!(figures.active, 1);
        assert_eq!(figures.queued, 1);
        assert_eq!(figures.bytes_per_second, 250);
        assert_eq!(figures.eta_seconds, Some(6));
    }

    /// An unknown size, a rate of zero and a paused transfer each leave the field empty
    /// rather than putting an invented number in front of the user (RD-104-02).
    #[test]
    fn no_honest_estimate_means_no_field_rather_than_a_placeholder() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let sizeless = file(DownloadState::Queued, 0, None);
        let table = rates(&[(&running, 250)]);
        assert_eq!(
            capture_figures(&[running.clone(), sizeless], &table).eta_seconds,
            None,
            "one entry of unknown size makes the sum a lower bound"
        );

        assert_eq!(
            capture_figures(
                std::slice::from_ref(&running),
                &std::collections::HashMap::new()
            )
            .eta_seconds,
            None,
            "a rate of zero is the service saying nothing is moving"
        );

        let paused = file(DownloadState::Paused, 100, Some(9_000_000));
        let figures = capture_figures(&[paused], &std::collections::HashMap::new());
        assert_eq!(figures.eta_seconds, None);
        assert_eq!(figures.bytes_per_second, 0);
    }

    /// The counts and the estimate draw their line differently, and this is the job that says
    /// so out loud: a resolving entry is in the remaining time and in neither count.
    #[test]
    fn the_estimate_spans_the_queue_while_the_counts_name_two_states() {
        let running = file(DownloadState::Downloading, 0, Some(1_000));
        let resolving = file(DownloadState::Resolving, 0, Some(1_000));
        let table = rates(&[(&running, 100)]);

        let figures = capture_figures(&[running.clone(), resolving], &table);

        assert_eq!(figures.active, 1);
        assert_eq!(figures.queued, 0);
        assert_eq!(
            figures.eta_seconds,
            Some(20),
            "the resolving entry's bytes are in the estimate although no count names it"
        );
    }

    /// A capture token is scoped for handing links in. The summary stays figures only: the
    /// serialized response must name no file, no path and no account.
    #[test]
    fn the_response_names_nothing_that_is_being_downloaded() {
        let running = file(DownloadState::Downloading, 500, Some(1_000));
        let table = rates(&[(&running, 250)]);

        let body = serde_json::to_string(&capture_figures(std::slice::from_ref(&running), &table))
            .expect("serialize the summary");

        assert!(!body.contains("a.bin"), "no file name: {body}");
        assert!(!body.contains("example.test"), "no source: {body}");
        assert!(!body.contains(&running.id.to_string()), "no id: {body}");
        assert!(
            body.contains("\"eta_seconds\":2"),
            "the estimate travels: {body}"
        );
    }
}
