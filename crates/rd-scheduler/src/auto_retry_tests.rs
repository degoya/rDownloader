//! RD-191-12 — limit waits that spend no attempts, and the automatic retry of failed downloads.
//!
//! Against a real database and a scheduler that admits no transfer (`max_active_files: 0`), so a
//! row that is queued stays queued and every state below is the retry's doing, not a worker's.
//! The passes are driven by hand with a clock of the test's choosing; the supervise loop would
//! run one only after a minute.

use std::{path::Path, sync::atomic::Ordering};

use chrono::{Duration, Utc};
use rd_core::{DownloadFile, DownloadId, DownloadState, Failure, FailureKind};

use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

async fn installation(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("auto-retry.sqlite3"))
        .await
        .expect("database");
    (start(directory, &database).await, database)
}

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

/// The settings switch, set on the handle directly: `update_runtime_settings` would refuse the
/// `max_active_files: 0` that keeps every row where the retry put it.
fn switch(scheduler: &SchedulerHandle, enabled: bool, interval_hours: u32, max_rounds: u32) {
    scheduler
        .auto_retry_failed
        .store(enabled, Ordering::Release);
    scheduler
        .auto_retry_interval_hours
        .store(interval_hours, Ordering::Release);
    scheduler
        .auto_retry_max_rounds
        .store(max_rounds, Ordering::Release);
}

/// One package of one paused file.
async fn file(scheduler: &SchedulerHandle, directory: &Path) -> DownloadFile {
    let spec = PackageSpec {
        name: format!("package {}", DownloadId::new()),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: true,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source: "https://hoster.example.invalid/file.bin"
            .parse()
            .expect("url"),
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

/// A file that has failed for good with `failure`.
async fn failed(
    scheduler: &SchedulerHandle,
    database: &rd_db::Database,
    directory: &Path,
    failure: Failure,
) -> DownloadFile {
    let file = file(scheduler, directory).await;
    database
        .record_failure(file.id, failure, None)
        .await
        .expect("record the failure")
}

async fn row(database: &rd_db::Database, id: DownloadId) -> DownloadFile {
    database.get_download(id).await.expect("read").expect("row")
}

async fn counters(database: &rd_db::Database, id: DownloadId) -> rd_db::RetryCounters {
    database
        .retry_counters(id)
        .await
        .expect("read")
        .expect("row")
}

/// The present to the second, so a due time reads back from the database exactly as written.
fn clock() -> chrono::DateTime<Utc> {
    chrono::DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("a valid time")
}

fn daily_limit() -> Failure {
    Failure::new(
        FailureKind::RateLimited {
            retry_after_seconds: None,
        },
        "daily limit reached",
    )
}

fn transient() -> Failure {
    Failure::new(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        "server busy",
    )
}

#[tokio::test]
async fn a_limit_wait_spends_no_attempt_and_any_other_failure_ends_the_run_of_waits() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let file = file(&scheduler, directory.path()).await;

    crate::failures::record_error(&scheduler, &file, daily_limit())
        .await
        .expect("record");
    let waiting = row(&database, file.id).await;
    assert_eq!(waiting.state, DownloadState::RetryWait);
    assert_eq!(waiting.retry_count, 0, "the limit wait spent an attempt");
    let due = waiting.next_retry_at.expect("a due time");
    let minutes = (due - Utc::now()).num_minutes();
    assert!(
        (59..=60).contains(&minutes),
        "waits {minutes} min, not the hour"
    );
    assert_eq!(counters(&database, file.id).await.limit_waits, 1);

    crate::failures::record_error(&scheduler, &waiting, transient())
        .await
        .expect("record");
    let retried = row(&database, file.id).await;
    assert_eq!(retried.retry_count, 1);
    assert_eq!(
        counters(&database, file.id).await.limit_waits,
        0,
        "the run of limit waits outlived another failure"
    );
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn the_limit_waits_end_after_their_bound_with_a_coded_failure() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let mut current = file(&scheduler, directory.path()).await;

    for _ in 0..crate::MAX_LIMIT_WAITS {
        crate::failures::record_error(&scheduler, &current, daily_limit())
            .await
            .expect("record");
        current = row(&database, current.id).await;
        assert_eq!(current.state, DownloadState::RetryWait);
    }
    assert_eq!(current.retry_count, 0);
    crate::failures::record_error(&scheduler, &current, daily_limit())
        .await
        .expect("record");
    let ended = row(&database, current.id).await;
    assert_eq!(ended.state, DownloadState::Failed);
    let failure = ended.last_error.expect("a failure");
    assert_eq!(
        failure.code.as_deref(),
        Some(crate::LIMIT_WAITS_EXHAUSTED_CODE)
    );
    assert!(failure.category.is_limit());
    assert_eq!(failure.params.get("waits").map(String::as_str), Some("48"));
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn the_retry_takes_up_only_failures_a_later_attempt_can_change() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let path = directory.path();
    let mut eligible = Vec::new();
    for failure in [
        transient(),
        daily_limit(),
        Failure::new(FailureKind::Offline, "unavailable for legal reasons"),
        Failure::new(
            FailureKind::IpBlocked {
                retry_after_seconds: None,
            },
            "one download at a time",
        ),
    ] {
        eligible.push(failed(&scheduler, &database, path, failure).await.id);
    }
    let mut never = Vec::new();
    for failure in [
        Failure::new(FailureKind::Permanent, "gone"),
        Failure::coded(
            FailureKind::Permanent,
            crate::finish::CHECKSUM_MISMATCH_CODE,
            "checksum mismatch",
        ),
        Failure::new(FailureKind::CaptchaFailed, "wrong captcha"),
        Failure::coded(
            FailureKind::Offline,
            crate::mirrors::HANDOVER_CODE,
            "this mirror was given up",
        ),
    ] {
        never.push(failed(&scheduler, &database, path, failure).await.id);
    }
    // Refused accounts, missing logins, captchas nobody solved and kinds nobody handles are
    // `blocked`, not `failed`: the retry does not even read them.
    let blocked = failed(
        &scheduler,
        &database,
        path,
        Failure::new(FailureKind::AccountInvalid, "wrong password"),
    )
    .await;
    assert_eq!(blocked.state, DownloadState::Blocked);

    switch(&scheduler, true, 6, 3);
    let now = clock();
    assert_eq!(scheduler.auto_retry_pass(now).await.expect("pass"), 0);
    for id in &eligible {
        let waiting = row(&database, *id).await;
        assert_eq!(waiting.state, DownloadState::Failed);
        assert_eq!(waiting.next_retry_at, Some(now + Duration::hours(6)));
    }
    for id in &never {
        assert_eq!(row(&database, *id).await.next_retry_at, None);
    }

    let later = now + Duration::hours(6) + Duration::minutes(1);
    assert_eq!(scheduler.auto_retry_pass(later).await.expect("pass"), 4);
    for id in &eligible {
        let queued = row(&database, *id).await;
        assert_eq!(queued.state, DownloadState::Queued);
        assert_eq!(queued.retry_count, 0, "the round kept the spent attempts");
        assert_eq!(queued.next_retry_at, None);
        assert_eq!(counters(&database, *id).await.auto_retry_rounds, 1);
    }
    for id in &never {
        let left = row(&database, *id).await;
        assert_eq!(left.state, DownloadState::Failed);
        assert_eq!(counters(&database, *id).await.auto_retry_rounds, 0);
    }
    assert_eq!(
        row(&database, blocked.id).await.state,
        DownloadState::Blocked
    );
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn the_retry_stops_after_its_rounds() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let file = failed(&scheduler, &database, directory.path(), transient()).await;
    switch(&scheduler, true, 1, 2);

    let mut now = clock();
    for round in 1..=2 {
        scheduler.auto_retry_pass(now).await.expect("stamp");
        now += Duration::hours(1);
        assert_eq!(scheduler.auto_retry_pass(now).await.expect("due"), 1);
        assert_eq!(row(&database, file.id).await.state, DownloadState::Queued);
        assert_eq!(counters(&database, file.id).await.auto_retry_rounds, round);
        database
            .record_failure(file.id, transient(), None)
            .await
            .expect("fail again");
    }
    scheduler.auto_retry_pass(now).await.expect("pass");
    now += Duration::hours(2);
    assert_eq!(scheduler.auto_retry_pass(now).await.expect("pass"), 0);
    let left = row(&database, file.id).await;
    assert_eq!(left.state, DownloadState::Failed);
    assert_eq!(left.next_retry_at, None, "a third round was announced");

    // `0` is no limit: the same row comes back once more.
    switch(&scheduler, true, 1, 0);
    scheduler.auto_retry_pass(now).await.expect("stamp");
    now += Duration::hours(1);
    assert_eq!(scheduler.auto_retry_pass(now).await.expect("due"), 1);
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn switched_off_it_does_nothing_and_takes_back_what_it_announced() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let file = failed(&scheduler, &database, directory.path(), daily_limit()).await;

    let now = clock();
    switch(&scheduler, false, 6, 3);
    for hours in [0, 7, 30] {
        assert_eq!(
            scheduler
                .auto_retry_pass(now + Duration::hours(hours))
                .await
                .expect("pass"),
            0
        );
    }
    let untouched = row(&database, file.id).await;
    assert_eq!(untouched.state, DownloadState::Failed);
    assert_eq!(untouched.next_retry_at, None);

    switch(&scheduler, true, 6, 3);
    scheduler.auto_retry_pass(now).await.expect("stamp");
    assert!(row(&database, file.id).await.next_retry_at.is_some());
    switch(&scheduler, false, 6, 3);
    assert_eq!(
        scheduler
            .auto_retry_pass(now + Duration::hours(7))
            .await
            .expect("pass"),
        0
    );
    let withdrawn = row(&database, file.id).await;
    assert_eq!(withdrawn.state, DownloadState::Failed);
    assert_eq!(withdrawn.next_retry_at, None, "the announced retry stayed");
    scheduler.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_shorter_interval_brings_the_due_time_nearer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let file = failed(&scheduler, &database, directory.path(), transient()).await;
    let now = clock();

    switch(&scheduler, true, 24, 3);
    scheduler.auto_retry_pass(now).await.expect("stamp");
    assert_eq!(
        row(&database, file.id).await.next_retry_at,
        Some(now + Duration::hours(24))
    );
    switch(&scheduler, true, 2, 3);
    scheduler.auto_retry_pass(now).await.expect("restamp");
    assert_eq!(
        row(&database, file.id).await.next_retry_at,
        Some(now + Duration::hours(2))
    );
    scheduler.shutdown().await.expect("shutdown");
}

/// A failed download resumed by hand starts with a fresh budget, uncounted as a round, and
/// so gets its automatic retries again; a paused one keeps its counters.
#[tokio::test]
async fn a_failed_download_resumed_by_hand_gets_its_retries_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let file = file(&scheduler, directory.path()).await;
    for _ in 0..crate::DEFAULT_MAX_RETRIES {
        database
            .record_failure(file.id, transient(), Some(Utc::now()))
            .await
            .expect("retry");
    }
    let failed = database
        .record_failure(file.id, transient(), None)
        .await
        .expect("give up");
    assert_eq!(failed.state, DownloadState::Failed);
    assert!(failed.retry_count > crate::DEFAULT_MAX_RETRIES);

    scheduler.resume(file.id).await.expect("resume");
    let resumed = row(&database, file.id).await;
    assert_eq!(resumed.state, DownloadState::Queued);
    assert_eq!(resumed.retry_count, 0, "the spent attempts came along");
    assert_eq!(resumed.next_retry_at, None);
    assert_eq!(
        counters(&database, file.id).await,
        rd_db::RetryCounters::default(),
        "a resume by hand counted a round"
    );

    crate::failures::record_error(&scheduler, &resumed, transient())
        .await
        .expect("record");
    assert_eq!(
        row(&database, file.id).await.state,
        DownloadState::RetryWait,
        "the next transient failure ended the download at once"
    );

    // Paused, the counters stay as they are.
    database
        .transition_download(file.id, DownloadState::Paused)
        .await
        .expect("pause");
    scheduler.resume(file.id).await.expect("resume");
    let unpaused = row(&database, file.id).await;
    assert_eq!(unpaused.state, DownloadState::Queued);
    assert_eq!(unpaused.retry_count, 1);
    scheduler.shutdown().await.expect("shutdown");
}

/// `scheduler.before_auto_retry_requeued`: the round came due, the row was not written yet.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_crash_before_the_requeue_leaves_the_round_to_the_next_start() {
    use rd_core::failpoint::FailpointGuard;

    let directory = tempfile::tempdir().expect("tempdir");
    let (scheduler, database) = installation(directory.path()).await;
    let file = failed(&scheduler, &database, directory.path(), transient()).await;
    switch(&scheduler, true, 1, 3);
    let now = clock();
    scheduler.auto_retry_pass(now).await.expect("stamp");

    let guard = FailpointGuard::once("scheduler.before_auto_retry_requeued");
    let due = now + Duration::hours(1);
    assert!(
        scheduler.auto_retry_pass(due).await.is_err(),
        "the crash point did not stop the pass"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);
    scheduler.shutdown().await.expect("shutdown");
    let stopped = row(&database, file.id).await;
    assert_eq!(stopped.state, DownloadState::Failed);
    assert_eq!(stopped.next_retry_at, Some(due));
    assert_eq!(counters(&database, file.id).await.auto_retry_rounds, 0);

    let restarted = start(directory.path(), &database).await;
    switch(&restarted, true, 1, 3);
    assert_eq!(restarted.auto_retry_pass(due).await.expect("pass"), 1);
    assert_eq!(restarted.auto_retry_pass(due).await.expect("pass"), 0);
    let queued = row(&database, file.id).await;
    assert_eq!(queued.state, DownloadState::Queued);
    assert_eq!(queued.retry_count, 0);
    let counted = counters(&database, file.id).await;
    assert_eq!(
        counted.auto_retry_rounds, 1,
        "the round was not counted once"
    );
    assert_eq!(counted.limit_waits, 0);
    restarted.shutdown().await.expect("shutdown");
}
