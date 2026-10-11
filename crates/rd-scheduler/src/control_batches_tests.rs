//! Pausing and resuming many files at once (RD-1250-08): the running ones stopped by their
//! tokens, the held ones refused, the rest through the one transaction.

use rd_core::{DownloadId, DownloadState};
use tokio_util::sync::CancellationToken;

use super::super::tests::paused_package;

/// A running file is stopped by its token and keeps its stop reason for its worker; a waiting
/// one is paused in the batch and lets its reason go; an unknown id is refused on its own.
#[tokio::test]
async fn a_pause_of_many_stops_the_running_files_and_pauses_the_waiting_ones() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    // Queued under a reason of its own, so the supervisor does not start it meanwhile.
    scheduler
        .active
        .lock()
        .await
        .reasons
        .insert(file.id, crate::StopReason::Paused);
    scheduler
        .database
        .transition_download(file.id, DownloadState::Queued)
        .await
        .expect("queued");
    let running = DownloadId::new();
    let token = CancellationToken::new();
    scheduler
        .active
        .lock()
        .await
        .tokens
        .insert(running, token.clone());
    let unknown = DownloadId::new();

    let answers = scheduler.pause_many(&[running, file.id, unknown]).await;
    assert!(answers[0].is_ok() && answers[1].is_ok(), "{answers:?}");
    assert_eq!(
        answers[2].as_ref().err().and_then(rd_db::store_kind),
        Some(rd_db::StoreErrorKind::NotFound)
    );
    assert!(token.is_cancelled(), "the running file was stopped");
    let state = scheduler
        .database
        .get_download(file.id)
        .await
        .expect("read")
        .expect("row")
        .state;
    assert_eq!(state, DownloadState::Paused);
    let mut active = scheduler.active.lock().await;
    assert!(
        active.reasons.contains_key(&running),
        "its worker writes the pause"
    );
    assert!(!active.reasons.contains_key(&file.id));
    assert!(!active.reasons.contains_key(&unknown));
    active.tokens.remove(&running);
    active.reasons.remove(&running);
}

/// A file a reset or removal holds is refused, as the single resume refuses it, and stays
/// paused; the stop reason of the others goes.
#[tokio::test]
async fn a_resume_of_many_refuses_a_held_file() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    scheduler.active.lock().await.held.insert(file.id, 1);
    let unknown = DownloadId::new();

    let answers = scheduler.resume_many(&[file.id, unknown]).await;
    let kinds: Vec<_> = answers
        .iter()
        .map(|answer| answer.as_ref().err().and_then(rd_db::store_kind))
        .collect();
    assert_eq!(
        kinds,
        [
            Some(rd_db::StoreErrorKind::WrongState),
            Some(rd_db::StoreErrorKind::NotFound)
        ]
    );
    let state = scheduler
        .database
        .get_download(file.id)
        .await
        .expect("read")
        .expect("row")
        .state;
    assert_eq!(state, DownloadState::Paused, "a held file is not queued");
    scheduler.active.lock().await.release_hold(&file.id);
}
