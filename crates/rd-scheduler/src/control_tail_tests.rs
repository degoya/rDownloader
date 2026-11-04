//! Removing a download right after its worker wrote the last state, while the worker still has
//! its place in `active` (RD-1120-18).

use std::time::Duration;

use rd_core::{DownloadFile, DownloadState};
use tokio_util::sync::CancellationToken;

use super::tests::paused_package;
use crate::SchedulerHandle;

/// Moves the row to `state` through the states a worker passes on the way.
async fn worked_to(scheduler: &SchedulerHandle, file: &DownloadFile, state: DownloadState) {
    for step in [DownloadState::Downloading, DownloadState::Verifying] {
        scheduler
            .database
            .transition_download(file.id, step)
            .await
            .expect("on its way");
        if step == state {
            return;
        }
    }
    scheduler
        .database
        .complete_download(file.id, file.file_name.clone(), None)
        .await
        .expect("complete");
}

/// The nightly soak of 2026-10-06: the list said "completed", the worker had not let go of the
/// file yet, and the forced removal of its package was answered with 409. It waits for the
/// worker's tail now and removes the file.
#[tokio::test]
async fn a_download_is_removable_right_after_it_completed() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    worked_to(&scheduler, &file, DownloadState::Completed).await;
    // The worker's tail: the row is finished, the token is still there.
    scheduler
        .active
        .lock()
        .await
        .tokens
        .insert(file.id, CancellationToken::new());
    let tail = {
        let scheduler = scheduler.clone();
        let id = file.id;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            scheduler.active.lock().await.tokens.remove(&id);
        })
    };

    scheduler
        .remove(file.id)
        .await
        .expect("removed once the worker let go");

    tail.await.expect("the tail ended");
    assert!(
        scheduler
            .database
            .get_download(file.id)
            .await
            .expect("lookup")
            .is_none(),
        "the finished download is gone"
    );
    assert!(
        !scheduler.active.lock().await.held.contains_key(&file.id),
        "and the hold went with the removal"
    );
}

/// The wait is for a worker in its tail only: one whose row still says it is at work is
/// refused at once, as it was.
#[tokio::test]
async fn a_worker_that_is_still_at_work_is_refused_at_once() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    worked_to(&scheduler, &file, DownloadState::Downloading).await;
    scheduler
        .active
        .lock()
        .await
        .tokens
        .insert(file.id, CancellationToken::new());

    let started = std::time::Instant::now();
    let refused = scheduler
        .while_held(file.id, "refused", async { anyhow::Ok(()) })
        .await
        .expect_err("a running worker keeps the file");

    assert_eq!(
        rd_db::store_kind(&refused),
        Some(rd_db::StoreErrorKind::WrongState)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "refused without waiting for the tail"
    );
    assert!(!scheduler.active.lock().await.held.contains_key(&file.id));
}

/// "Remove failed packages + files": a corrupt Usenet set left its finished RAR volumes on disk,
/// because a removal only ever discarded what had not finished. `discard_all` takes the
/// finished payload too; `discard_partial` still leaves it.
#[tokio::test]
async fn discarding_all_data_takes_the_finished_payload_too() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, destination) = paused_package(temporary.path()).await;
    worked_to(&scheduler, &file, DownloadState::Completed).await;
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let payload = destination.join(&file.file_name);
    tokio::fs::write(&payload, b"volume")
        .await
        .expect("payload");

    scheduler
        .discard_partial(file.id)
        .await
        .expect("discard partial");
    assert!(payload.exists(), "a finished file is not partial data");

    scheduler.discard_all(file.id).await.expect("discard all");
    assert!(!payload.exists(), "the whole package's data goes");
}

/// A resume that reaches a worker before it stopped is refused by the row's state; it must
/// leave the stop it met alone, or the worker writes the fallback `Paused` instead of the
/// cancel that was asked for.
#[tokio::test]
async fn a_refused_resume_keeps_the_stop_a_worker_is_ending_with() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    worked_to(&scheduler, &file, DownloadState::Downloading).await;
    scheduler
        .active
        .lock()
        .await
        .tokens
        .insert(file.id, CancellationToken::new());

    scheduler.cancel(file.id).await.expect("cancel requested");
    let refused = scheduler
        .resume(file.id)
        .await
        .expect_err("a running row cannot be queued");
    assert_eq!(
        rd_db::store_kind(&refused),
        Some(rd_db::StoreErrorKind::WrongState)
    );

    // The worker reaches its checkpoint.
    let row = scheduler
        .database
        .get_download(file.id)
        .await
        .expect("lookup")
        .expect("row");
    crate::worker::transition_stopped(&scheduler, &row)
        .await
        .expect("stopped");
    let state = scheduler
        .database
        .get_download(file.id)
        .await
        .expect("lookup")
        .expect("row")
        .state;
    assert_eq!(state, DownloadState::Cancelled);
}

/// In the worker's tail the row already says `Paused`, and a resume queues it as before.
#[tokio::test]
async fn a_resume_in_the_worker_tail_queues_the_paused_row() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, file, _destination) = paused_package(temporary.path()).await;
    scheduler
        .active
        .lock()
        .await
        .tokens
        .insert(file.id, CancellationToken::new());

    scheduler.resume(file.id).await.expect("resumed");
    let state = scheduler
        .database
        .get_download(file.id)
        .await
        .expect("lookup")
        .expect("row")
        .state;
    assert_eq!(state, DownloadState::Queued);
}
