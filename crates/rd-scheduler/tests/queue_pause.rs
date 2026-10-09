//! The timed pause of the whole queue (RD-190-20): what it stops, what its end lets go, and
//! what a restart makes of it — including the crash between its record and its files.
//!
//! The scheduler here admits no transfer at all (`max_active_files: 0`), so a file that is
//! queued stays queued and every state below is the pause's doing, not a race with a worker.

use std::{path::Path, time::Duration};

use chrono::Utc;
use rd_core::{DownloadFile, DownloadId, DownloadState};
use rd_scheduler::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

async fn start(directory: &Path, database: &rd_db::Database) -> SchedulerHandle {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: 0,
            ..SchedulerConfig::for_directory(directory.join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler")
}

async fn installation(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("queue-pause.sqlite3"))
        .await
        .expect("database");
    (start(directory, &database).await, database)
}

/// One package of one file, queued or created paused.
async fn file(scheduler: &SchedulerHandle, directory: &Path, paused: bool) -> DownloadFile {
    let spec = PackageSpec {
        name: format!("package {}", DownloadId::new()),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: paused,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source: "https://example.invalid/file.bin".parse().expect("url"),
        file_name: "file.bin".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }];
    let (_, files) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    files.into_iter().next().expect("one file")
}

async fn state(database: &rd_db::Database, id: DownloadId) -> DownloadState {
    database
        .get_download(id)
        .await
        .expect("read")
        .expect("row")
        .state
}

/// Waits for the supervise loop, which ticks twice a second.
async fn eventually(database: &rd_db::Database, id: DownloadId, wanted: DownloadState) {
    for _ in 0..100 {
        if state(database, id).await == wanted {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{id} never became {wanted:?}");
}

#[tokio::test]
async fn the_end_resumes_what_the_pause_stopped_and_nothing_else() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let first = file(&scheduler, directory.path(), false).await;
    let second = file(&scheduler, directory.path(), false).await;
    let paused_before = file(&scheduler, directory.path(), true).await;

    let until = Utc::now() + chrono::Duration::hours(1);
    let pause = scheduler.pause_queue_until(until).await.expect("pause");

    assert_eq!(pause.until, Some(until));
    assert_eq!(pause.files.len(), 2, "{pause:?}");
    assert!(!pause.files.contains(&paused_before.id));
    assert_eq!(state(&database, first.id).await, DownloadState::Paused);
    assert_eq!(state(&database, second.id).await, DownloadState::Paused);
    assert_eq!(scheduler.network_hold().await, Some("queue_paused"));

    let resumed = scheduler.resume_queue().await.expect("resume");

    assert_eq!(resumed, 2);
    assert_eq!(state(&database, first.id).await, DownloadState::Queued);
    assert_eq!(state(&database, second.id).await, DownloadState::Queued);
    assert_eq!(
        state(&database, paused_before.id).await,
        DownloadState::Paused,
        "a file paused before the queue was must stay paused"
    );
    assert_eq!(scheduler.network_hold().await, None);
    assert_eq!(scheduler.queue_pause().await, None);
    assert_eq!(scheduler.resume_queue().await.expect("again"), 0);
}

#[tokio::test]
async fn pausing_again_moves_the_end_and_keeps_the_files() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, _database) = installation(directory.path()).await;
    let first = file(&scheduler, directory.path(), false).await;
    scheduler
        .pause_queue_until(Utc::now() + chrono::Duration::minutes(30))
        .await
        .expect("pause");
    // Added under the pause: it waits behind the hold, and the next pause takes it in.
    let added = file(&scheduler, directory.path(), false).await;

    let later = Utc::now() + chrono::Duration::hours(3);
    let pause = scheduler.pause_queue_until(later).await.expect("again");

    assert_eq!(pause.until, Some(later));
    assert!(pause.files.contains(&first.id), "{pause:?}");
    assert!(pause.files.contains(&added.id), "{pause:?}");
    assert_eq!(scheduler.queue_pause().await, Some(pause));
}

#[tokio::test]
async fn a_pause_survives_a_restart_until_its_end() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let queued = file(&scheduler, directory.path(), false).await;
    let until = Utc::now() + chrono::Duration::hours(1);
    scheduler.pause_queue_until(until).await.expect("pause");
    scheduler.shutdown().await.expect("shutdown");

    let restarted = start(directory.path(), &database).await;

    let pause = restarted.queue_pause().await.expect("the pause came back");
    assert_eq!(pause.until, Some(until));
    assert_eq!(restarted.network_hold().await, Some("queue_paused"));
    assert_eq!(state(&database, queued.id).await, DownloadState::Paused);
}

#[tokio::test]
async fn an_end_that_passed_while_the_service_was_down_resumes_at_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let queued = file(&scheduler, directory.path(), false).await;
    scheduler
        .pause_queue_until(Utc::now() + chrono::Duration::hours(1))
        .await
        .expect("pause");
    scheduler.shutdown().await.expect("shutdown");
    assert_eq!(state(&database, queued.id).await, DownloadState::Paused);
    // The end passes while the service is down: moved into the past in the record the next
    // start reads, instead of waiting for a real one to pass (RD-1120-08).
    let mut stored = database
        .get_setting("queue.timed_pause")
        .await
        .expect("read the pause")
        .expect("the stored pause");
    stored["until"] =
        serde_json::to_value(Utc::now() - chrono::Duration::seconds(1)).expect("an end");
    database
        .set_setting("queue.timed_pause".to_owned(), stored)
        .await
        .expect("move the end");

    let restarted = start(directory.path(), &database).await;

    eventually(&database, queued.id, DownloadState::Queued).await;
    for _ in 0..50 {
        if restarted.queue_pause().await.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(restarted.queue_pause().await, None);
    assert_eq!(restarted.network_hold().await, None);
}

/// `scheduler.after_queue_pause_recorded`: the record is written, the files are not paused.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_crash_between_the_record_and_the_files_still_holds_them_until_the_end() {
    use rd_core::failpoint::FailpointGuard;

    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let queued = file(&scheduler, directory.path(), false).await;

    let guard = FailpointGuard::once("scheduler.after_queue_pause_recorded");
    let until = Utc::now() + chrono::Duration::seconds(3);
    assert!(
        scheduler.pause_queue_until(until).await.is_err(),
        "the crash point did not stop the pause"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);
    scheduler.shutdown().await.expect("shutdown");
    assert_eq!(
        state(&database, queued.id).await,
        DownloadState::Queued,
        "the file was paused although the run stopped before it"
    );

    // The next start: the record brings the hold back before anything is dispatched.
    let restarted = start(directory.path(), &database).await;
    let pause = restarted.queue_pause().await.expect("the record was lost");
    assert!(pause.files.contains(&queued.id), "{pause:?}");
    assert_eq!(restarted.network_hold().await, Some("queue_paused"));

    // And the end lets go of it: no hold, no record, the file still queued and free to start.
    for _ in 0..80 {
        if restarted.queue_pause().await.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        restarted.queue_pause().await,
        None,
        "the pause outlived its end"
    );
    assert_eq!(restarted.network_hold().await, None);
    assert_eq!(state(&database, queued.id).await, DownloadState::Queued);
}
