//! The queue's stop mark (RD-1210-02): it acts once its file or its whole package is done, lets
//! what runs finish, follows its target through a reorder, goes with it when it is deleted and
//! survives a restart — including the crash between the pause it sets and its own clearing.
//!
//! The scheduler here admits no transfer at all (`max_active_files: 0`), so a file moves only
//! when the test moves it: "running" and "done" are states written here, and every pause is
//! the mark's doing, not a race with a worker.

use std::{path::Path, time::Duration};

use rd_core::{DownloadFile, DownloadId, DownloadState, EventKind};
use rd_db::{StopMarkTarget, StoreErrorKind};
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
    let database = rd_db::Database::open(directory.join("stop-mark.sqlite3"))
        .await
        .expect("database");
    (start(directory, &database).await, database)
}

/// One package of `count` queued files.
async fn package(scheduler: &SchedulerHandle, directory: &Path, count: usize) -> Vec<DownloadFile> {
    let spec = PackageSpec {
        name: format!("package {}", DownloadId::new()),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: false,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = (0..count)
        .map(|index| FileSpec {
            source: format!("https://example.invalid/{}/{index}.bin", DownloadId::new())
                .parse()
                .expect("url"),
            file_name: format!("file-{index}.bin"),
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
        })
        .collect();
    scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue")
        .1
}

async fn file(scheduler: &SchedulerHandle, directory: &Path) -> DownloadFile {
    package(scheduler, directory, 1)
        .await
        .into_iter()
        .next()
        .expect("one file")
}

async fn state(database: &rd_db::Database, id: DownloadId) -> DownloadState {
    database
        .get_download(id)
        .await
        .expect("read")
        .expect("row")
        .state
}

/// Walks a queued file through the states a worker would write.
async fn walk(database: &rd_db::Database, id: DownloadId, states: &[DownloadState]) {
    for next in states {
        database
            .transition_download(id, *next)
            .await
            .expect("transition");
    }
}

async fn run(database: &rd_db::Database, id: DownloadId) {
    walk(
        database,
        id,
        &[DownloadState::Resolving, DownloadState::Downloading],
    )
    .await;
}

async fn complete(database: &rd_db::Database, id: DownloadId) {
    walk(
        database,
        id,
        &[
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
            DownloadState::Completed,
        ],
    )
    .await;
}

/// Waits for the supervise loop, which ticks twice a second, to clear the mark.
async fn reached(scheduler: &SchedulerHandle) {
    for _ in 0..100 {
        if scheduler.stop_mark().await.expect("read").is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the stop mark was never acted on");
}

/// A few ticks of the supervise loop, for "nothing happened".
async fn ticks() {
    tokio::time::sleep(Duration::from_millis(1_500)).await;
}

/// How many `queue.stop_mark` events said "reached".
fn reached_events(events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>) -> usize {
    let mut count = 0;
    while let Ok(event) = events.try_recv() {
        if event.kind == EventKind::QueueStopMark && event.payload["action"] == "reached" {
            count += 1;
        }
    }
    count
}

#[tokio::test]
async fn a_file_mark_pauses_the_queue_once_and_lets_running_files_finish() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let mut events = database.subscribe();
    let marked = file(&scheduler, directory.path()).await;
    let running = file(&scheduler, directory.path()).await;
    let waiting = file(&scheduler, directory.path()).await;
    run(&database, running.id).await;

    scheduler
        .set_stop_mark(StopMarkTarget::Download(marked.id))
        .await
        .expect("set");
    ticks().await;
    assert!(
        scheduler.queue_pause().await.is_none(),
        "a mark whose file still waits paused the queue"
    );

    complete(&database, marked.id).await;
    reached(&scheduler).await;

    let pause = scheduler.queue_pause().await.expect("the queue is paused");
    assert_eq!(pause.until, None, "the stop mark's pause has no end");
    assert!(pause.files.contains(&waiting.id), "{pause:?}");
    assert!(!pause.files.contains(&running.id), "{pause:?}");
    assert_eq!(state(&database, waiting.id).await, DownloadState::Paused);
    assert_eq!(
        state(&database, running.id).await,
        DownloadState::Downloading,
        "a running file must finish, not be paused"
    );
    assert_eq!(scheduler.network_hold().await, Some("queue_paused"));

    ticks().await;
    assert_eq!(
        reached_events(&mut events),
        1,
        "the mark acted more than once"
    );

    // The pause lasts until somebody resumes the queue, and that lets the waiting file go.
    assert_eq!(scheduler.resume_queue().await.expect("resume"), 1);
    assert_eq!(state(&database, waiting.id).await, DownloadState::Queued);
    assert_eq!(scheduler.network_hold().await, None);
}

#[tokio::test]
async fn a_package_mark_waits_for_its_last_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let members = package(&scheduler, directory.path(), 2).await;
    let other = file(&scheduler, directory.path()).await;
    let package_id = members[0].package_id;

    scheduler
        .set_stop_mark(StopMarkTarget::Package(package_id))
        .await
        .expect("set");
    complete(&database, members[0].id).await;
    ticks().await;
    assert!(
        scheduler.stop_mark().await.expect("read").is_some(),
        "the mark acted while a file of its package still waited"
    );
    assert!(scheduler.queue_pause().await.is_none());

    // Failed for good is done as well.
    walk(
        &database,
        members[1].id,
        &[DownloadState::Resolving, DownloadState::Failed],
    )
    .await;
    reached(&scheduler).await;

    assert!(scheduler.queue_pause().await.is_some());
    assert_eq!(state(&database, other.id).await, DownloadState::Paused);
}

#[tokio::test]
async fn the_mark_follows_its_file_through_a_reorder() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let members = package(&scheduler, directory.path(), 2).await;
    let (first, second) = (members[0].id, members[1].id);
    let package_id = members[0].package_id;
    let behind = file(&scheduler, directory.path()).await;

    scheduler
        .set_stop_mark(StopMarkTarget::Download(first))
        .await
        .expect("set");
    database
        .reorder_downloads(package_id, vec![second, first])
        .await
        .expect("reorder files");
    database
        .reorder_packages(vec![behind.package_id, package_id])
        .await
        .expect("reorder packages");

    let mark = scheduler.stop_mark().await.expect("read").expect("kept");
    assert_eq!(mark.target, StopMarkTarget::Download(first));
    // The file now at the marked file's old place finishing is no reason to stop.
    complete(&database, second).await;
    ticks().await;
    assert!(scheduler.queue_pause().await.is_none());

    complete(&database, first).await;
    reached(&scheduler).await;
    assert!(scheduler.queue_pause().await.is_some());
}

#[tokio::test]
async fn deleting_the_marked_file_clears_the_mark_and_pauses_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let marked = file(&scheduler, directory.path()).await;
    let waiting = file(&scheduler, directory.path()).await;
    scheduler
        .set_stop_mark(StopMarkTarget::Download(marked.id))
        .await
        .expect("set");

    database.delete_download(marked.id).await.expect("delete");

    assert_eq!(scheduler.stop_mark().await.expect("read"), None);
    ticks().await;
    assert!(scheduler.queue_pause().await.is_none());
    assert_eq!(state(&database, waiting.id).await, DownloadState::Queued);
}

#[tokio::test]
async fn a_mark_on_a_finished_or_missing_target_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let done = file(&scheduler, directory.path()).await;
    complete(&database, done.id).await;

    let refused = scheduler
        .set_stop_mark(StopMarkTarget::Download(done.id))
        .await
        .expect_err("a finished file");
    assert_eq!(
        rd_db::store_kind(&refused),
        Some(StoreErrorKind::WrongState)
    );
    let refused = scheduler
        .set_stop_mark(StopMarkTarget::Package(done.package_id))
        .await
        .expect_err("a finished package");
    assert_eq!(
        rd_db::store_kind(&refused),
        Some(StoreErrorKind::WrongState)
    );
    let refused = scheduler
        .set_stop_mark(StopMarkTarget::Download(DownloadId::new()))
        .await
        .expect_err("no such file");
    assert_eq!(rd_db::store_kind(&refused), Some(StoreErrorKind::NotFound));
    assert_eq!(scheduler.stop_mark().await.expect("read"), None);
}

#[tokio::test]
async fn a_new_mark_replaces_the_old_one_and_clearing_removes_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, _database) = installation(directory.path()).await;
    let first = file(&scheduler, directory.path()).await;
    let second = file(&scheduler, directory.path()).await;

    scheduler
        .set_stop_mark(StopMarkTarget::Download(first.id))
        .await
        .expect("set");
    scheduler
        .set_stop_mark(StopMarkTarget::Package(second.package_id))
        .await
        .expect("replace");
    let mark = scheduler.stop_mark().await.expect("read").expect("a mark");
    assert_eq!(mark.target, StopMarkTarget::Package(second.package_id));

    assert!(scheduler.clear_stop_mark().await.expect("clear"));
    assert!(!scheduler.clear_stop_mark().await.expect("nothing left"));
}

#[tokio::test]
async fn the_mark_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let marked = file(&scheduler, directory.path()).await;
    scheduler
        .set_stop_mark(StopMarkTarget::Download(marked.id))
        .await
        .expect("set");
    scheduler.shutdown().await.expect("shutdown");

    let restarted = start(directory.path(), &database).await;

    let mark = restarted.stop_mark().await.expect("read").expect("kept");
    assert_eq!(mark.target, StopMarkTarget::Download(marked.id));
    complete(&database, marked.id).await;
    reached(&restarted).await;
    assert!(restarted.queue_pause().await.is_some());
}

/// `scheduler.after_stop_mark_paused`: the pause is recorded and the waiting files are paused,
/// the mark is not cleared yet. The next start holds the queue before its first dispatch, acts on
/// the mark once more — which changes nothing about the pause — and says so once.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_crash_between_the_stop_mark_pause_and_its_clearing_acts_once_after_the_restart() {
    use rd_core::failpoint::FailpointGuard;

    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let marked = file(&scheduler, directory.path()).await;
    let waiting = file(&scheduler, directory.path()).await;
    scheduler
        .set_stop_mark(StopMarkTarget::Download(marked.id))
        .await
        .expect("set");

    // Every pass stops at the point until the "crash", so no later tick finishes the work.
    let guard = FailpointGuard::once("scheduler.after_stop_mark_paused");
    rd_core::failpoint::arm("scheduler.after_stop_mark_paused", u32::MAX);
    complete(&database, marked.id).await;
    for _ in 0..50 {
        if guard.fired() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(guard.fired(), "the crash point was never reached");
    scheduler.shutdown().await.expect("shutdown");
    // A tick already under way when the loop was cancelled still meets the armed point; only
    // once it is through may the point go, or that tick would finish what the crash stopped.
    tokio::time::sleep(Duration::from_millis(1_000)).await;
    drop(guard);
    assert!(
        database.stop_mark().await.expect("read").is_some(),
        "the mark was cleared although the run stopped before it"
    );
    assert_eq!(state(&database, waiting.id).await, DownloadState::Paused);

    let mut events = database.subscribe();
    let restarted = start(directory.path(), &database).await;
    assert_eq!(
        restarted.network_hold().await,
        Some("queue_paused"),
        "the hold was not back before the first dispatch"
    );
    reached(&restarted).await;
    let pause = restarted.queue_pause().await.expect("still paused");
    assert_eq!(pause.until, None);
    assert!(pause.files.contains(&waiting.id), "{pause:?}");
    assert_eq!(state(&database, waiting.id).await, DownloadState::Paused);
    ticks().await;
    assert_eq!(reached_events(&mut events), 1);
}
