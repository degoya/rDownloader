//! The queue's figures without its rows (RD-1480-19), and the events that tell a client holding
//! the list that a row changed in more than its bytes (RD-1480-18).

use std::collections::HashMap;

use rd_core::{DownloadFigures, DownloadId, DownloadPriority, DownloadState, EventKind};

use super::download_batches::package;

#[tokio::test]
async fn the_figures_say_what_the_rows_say() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    let (_, rows) = package(
        &database,
        directory.path(),
        "figures",
        DownloadPriority::Normal,
        3,
    )
    .await;
    database
        .transition_download(rows[1], DownloadState::Resolving)
        .await
        .expect("resolving");
    database
        .set_download_progress(rows[1], 400, Some(1_000))
        .await
        .expect("progress");

    let whole: HashMap<DownloadId, DownloadFigures> = database
        .list_downloads()
        .await
        .expect("list")
        .iter()
        .map(|file| (file.id, file.into()))
        .collect();
    let figures: HashMap<DownloadId, DownloadFigures> = database
        .download_figures()
        .await
        .expect("figures")
        .into_iter()
        .map(|figures| (figures.id, figures))
        .collect();
    assert_eq!(figures, whole);
    let working = figures[&rows[1]];
    assert_eq!(working.state, DownloadState::Resolving);
    assert_eq!(working.committed_bytes.get(), 400);
    assert_eq!(
        working.total_bytes.map(rd_core::ByteCount::get),
        Some(1_000)
    );
}

/// A client that reads progress as figures alone learns of a settled name, a chosen sign-in
/// profile and a recording's segments from an event that says the row itself changed.
#[tokio::test]
async fn a_row_that_changes_beyond_its_bytes_says_so() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    let (_, rows) = package(
        &database,
        directory.path(),
        "rows",
        DownloadPriority::Normal,
        1,
    )
    .await;
    let mut events = database.subscribe();

    database
        .set_download_file_name(rows[0], "settled.bin".to_owned())
        .await
        .expect("name");
    let renamed = events.try_recv().expect("event");
    assert_eq!(renamed.kind, EventKind::DownloadState);
    assert_eq!(
        renamed.payload,
        serde_json::json!({ "download_id": rows[0], "renamed": true }),
        "no state pair: no lifecycle moment"
    );

    database
        .set_download_auth_profile(rows[0], rd_core::AuthProfileSelection::None)
        .await
        .expect("profile");
    let profile = events.try_recv().expect("event");
    assert_eq!(profile.kind, EventKind::DownloadState);
    assert_eq!(profile.payload["auth_profile"], true);

    database
        .set_download_recording_state(rows[0], rd_core::RecordingState::default())
        .await
        .expect("recording");
    let recording = events.try_recv().expect("event");
    assert_eq!(recording.kind, EventKind::DownloadProgress);
    assert_eq!(recording.payload["recording"], true);

    database
        .set_download_progress(rows[0], 10, None)
        .await
        .expect("progress");
    let progress = events.try_recv().expect("event");
    assert_eq!(progress.kind, EventKind::DownloadProgress);
    assert_eq!(
        progress.payload,
        serde_json::json!({ "download_id": rows[0] }),
        "bytes alone carry no flag"
    );
}
