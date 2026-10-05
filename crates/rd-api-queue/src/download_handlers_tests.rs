use super::{is_transferring, moving_rates, queue_rate, remaining_of};
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
fn a_finished_download_keeps_no_rate() {
    let running = file(DownloadState::Downloading, 500, Some(1_000));
    let finished = file(DownloadState::Completed, 1_000, Some(1_000));
    let verifying = file(DownloadState::Verifying, 1_000, Some(1_000));
    let table = rates(&[(&running, 250), (&finished, 36_000_000), (&verifying, 10)]);

    let moving = moving_rates(table, &[running.clone(), finished, verifying]);

    assert_eq!(moving, HashMap::from([(running.id, 250)]));
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

/// A batch reports each refusal with the code its single endpoint answers with, so the
/// interface can translate it instead of showing the English text or nothing.
#[test]
fn a_bulk_refusal_carries_the_code_of_its_single_endpoint() {
    use crate::dto::DownloadBulkAction;
    use rd_db::StoreError;

    let refusal = |action, error: StoreError| {
        super::bulk_refusal(action, anyhow::Error::new(error)).into_message()
    };
    assert_eq!(
        refusal(
            DownloadBulkAction::Cancel,
            StoreError::wrong_state("cannot be cancelled")
        )
        .code,
        "download.cancel_state"
    );
    assert_eq!(
        refusal(
            DownloadBulkAction::Remove,
            StoreError::wrong_state("still running")
        )
        .code,
        "download.active_must_pause"
    );
    assert_eq!(
        refusal(DownloadBulkAction::Pause, StoreError::not_found("gone")).code,
        "download.not_found"
    );
    assert_eq!(
        refusal(
            DownloadBulkAction::Pause,
            StoreError::wrong_state("invalid download transition")
        )
        .code,
        "download.pause_state"
    );
    assert_eq!(
        refusal(
            DownloadBulkAction::Resume,
            StoreError::wrong_state("invalid download transition")
        )
        .code,
        "download.resume_state"
    );
    let taken = anyhow::Error::new(rd_scheduler::mirrors::MirrorTaken {
        source: "https://mirror.example/file".to_owned(),
    });
    assert_eq!(
        super::bulk_refusal(DownloadBulkAction::Resume, taken)
            .into_message()
            .code,
        "download.mirror_active"
    );
    // RA-DB-01: a reset of a Usenet file without its NZB says so, alone and in a batch.
    for action in [
        DownloadBulkAction::Reset,
        DownloadBulkAction::ResetDeleteFiles,
    ] {
        assert_eq!(
            super::bulk_refusal(action, anyhow::Error::new(rd_scheduler::NzbDropped))
                .into_message()
                .code,
            "download.nzb_dropped"
        );
    }
    assert_eq!(
        super::reset_failure(anyhow::Error::new(rd_scheduler::NzbDropped))
            .into_message()
            .code,
        "download.nzb_dropped"
    );
    // Anything untagged stays the internal error it is.
    assert_eq!(
        super::bulk_refusal(DownloadBulkAction::Resume, anyhow::anyhow!("disk on fire"))
            .into_message()
            .code,
        crate::error_codes::INTERNAL_ERROR
    );
}
