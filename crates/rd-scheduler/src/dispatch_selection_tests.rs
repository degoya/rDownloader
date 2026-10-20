//! RD-1250-07 -- the order the real dispatch pass starts waiting files in, on a fixed clock.
//!
//! The supervisor's own passes run on the wall clock, so a power hold keeps them out, and each
//! case drives the pass behind the hold with a `now` of its own. A stand-in FTP runner holds
//! every file it is given until it is cancelled, so a started file keeps its place for as long
//! as the case looks at it.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use rd_core::{DownloadFile, DownloadId, DownloadKind, DownloadPackage, Failure, FailureKind};
use tokio_util::sync::CancellationToken;

use crate::{
    ExternalRunner, FileSpec, HoldSource, PackageSpec, RunLimits, RunOutcome, SchedulerConfig,
    SchedulerHandle,
};

/// Holds every file until it is cancelled.
struct Holding;

#[async_trait::async_trait]
impl ExternalRunner for Holding {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Ftp
    }

    fn slot_capacity(&self) -> usize {
        4
    }

    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability::default()
    }

    async fn run(
        &self,
        _file: &DownloadFile,
        _package: &DownloadPackage,
        cancellation: CancellationToken,
        _limits: RunLimits,
    ) -> anyhow::Result<RunOutcome> {
        cancellation.cancelled().await;
        Ok(RunOutcome::Stopped)
    }
}

/// A scheduler with `parallel` places whose supervisor starts nothing.
async fn queue(directory: &Path, parallel: usize) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("order.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig {
            max_active_files: parallel,
            ..SchedulerConfig::for_directory(directory.join("downloads"))
        },
        secrets,
        None,
        vec![Arc::new(Holding)],
    )
    .await
    .expect("scheduler");
    scheduler
        .set_network_hold(HoldSource::Power, Some("test.fixed_clock"))
        .await;
    (scheduler, database)
}

/// One package of one file per name, enqueued in this order: the list order.
async fn packages(
    scheduler: &SchedulerHandle,
    directory: &Path,
    names: &[&str],
) -> Vec<DownloadFile> {
    let mut files = Vec::new();
    for name in names {
        let spec = PackageSpec {
            name: (*name).to_owned(),
            destination: directory.join("storage"),
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            password: None,
            start_paused: false,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        };
        let file = FileSpec {
            source: format!("ftp://files.example/{name}.mkv")
                .parse()
                .expect("url"),
            file_name: format!("{name}.mkv"),
            size: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::default(),
            kind: DownloadKind::Ftp,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            skipped: false,
            enrichment: Vec::new(),
            secret_fragment: None,
            source_set: None,
        };
        let (_, created) = scheduler
            .enqueue_package(spec, vec![file])
            .await
            .expect("enqueue");
        files.extend(created);
    }
    files
}

/// The attempt on `file` failed with `category` and waits until `at`.
async fn waits(
    database: &rd_db::Database,
    file: &DownloadFile,
    category: FailureKind,
    at: DateTime<Utc>,
) {
    database
        .record_failure(file.id, Failure::new(category, "refused"), Some(at))
        .await
        .expect("failure");
}

async fn pass(scheduler: &SchedulerHandle, now: DateTime<Utc>) {
    scheduler
        .dispatch_pass(now, &mut HashMap::new())
        .await
        .expect("pass");
}

async fn running(scheduler: &SchedulerHandle) -> HashSet<DownloadId> {
    scheduler
        .active
        .lock()
        .await
        .tokens
        .keys()
        .copied()
        .collect()
}

fn transient() -> FailureKind {
    FailureKind::Transient {
        retry_after_seconds: None,
    }
}

/// The tester's queue in small: parallel 1, and the attempt on the first file failed and waits
/// seconds for its plain retry. Its place went to the next package's file at once, which then
/// held it for a whole transfer; every further failure moved the place on and left a begun file
/// behind. Now it keeps the place by retrying, and no file after it takes it meanwhile.
#[tokio::test]
async fn a_file_waiting_for_its_retry_keeps_its_place_against_the_files_after_it() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = queue(directory.path(), 1).await;
    let files = packages(&scheduler, directory.path(), &["first", "next"]).await;
    let now = Utc::now();
    waits(
        &database,
        &files[0],
        transient(),
        now + chrono::Duration::seconds(4),
    )
    .await;

    pass(&scheduler, now).await;
    assert!(
        running(&scheduler).await.is_empty(),
        "the next package's file took the place the waiting file comes back to"
    );

    pass(&scheduler, now + chrono::Duration::seconds(5)).await;
    assert_eq!(running(&scheduler).await, HashSet::from([files[0].id]));
    scheduler.shutdown().await.expect("shutdown");
}

/// Pauses `id`, a running file, and waits until its place is free again.
async fn end(scheduler: &SchedulerHandle, id: DownloadId) {
    scheduler.pause(id).await.expect("pause");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !running(scheduler).await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the place never came back"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A wait the hoster stated -- a minute, "temporarily unavailable" -- does not stall the queue:
/// the file after it starts. Once the wait is over and a place is free, the file that waited
/// starts before the files queued after it.
#[tokio::test]
async fn a_stated_wait_leaves_its_place_and_comes_back_before_later_files() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = queue(directory.path(), 1).await;
    let files = packages(
        &scheduler,
        directory.path(),
        &["unavailable", "second", "third"],
    )
    .await;
    let now = Utc::now();
    let minute = chrono::Duration::seconds(60);
    waits(
        &database,
        &files[0],
        FailureKind::Transient {
            retry_after_seconds: Some(60),
        },
        now + minute,
    )
    .await;

    pass(&scheduler, now).await;
    assert_eq!(running(&scheduler).await, HashSet::from([files[1].id]));

    // The second file ends; its place is free again.
    end(&scheduler, files[1].id).await;
    pass(&scheduler, now + minute + chrono::Duration::seconds(1)).await;
    assert_eq!(
        running(&scheduler).await,
        HashSet::from([files[0].id]),
        "the file that waited comes before the one queued after it"
    );
    scheduler.shutdown().await.expect("shutdown");
}

/// The sixth failed attempt in a row waits the middle stage's first minute, through the real
/// failure path, and the place moves on to the next file instead of being taken again every few
/// seconds (RD-1250-07, RD-1340-01).
#[tokio::test]
async fn the_sixth_failure_in_a_row_waits_a_minute_and_the_place_moves_on() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = queue(directory.path(), 1).await;
    let files = packages(&scheduler, directory.path(), &["broken", "next"]).await;
    let now = Utc::now();
    for _ in 0..crate::retry::FAST_RETRIES {
        waits(&database, &files[0], transient(), now).await;
    }
    let broken = database
        .get_download(files[0].id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(broken.retry_count, crate::retry::FAST_RETRIES);
    crate::failures::record_error(&scheduler, &broken, Failure::new(transient(), "refused"))
        .await
        .expect("sixth failure");
    let waiting = database
        .get_download(files[0].id)
        .await
        .expect("read")
        .expect("row");
    let wait = waiting.next_retry_at.expect("still retried") - now;
    assert!(
        (chrono::Duration::seconds(50)..=chrono::Duration::seconds(70)).contains(&wait),
        "the sixth failure waits {wait}, not a minute"
    );

    pass(&scheduler, now).await;
    assert_eq!(running(&scheduler).await, HashSet::from([files[1].id]));
    scheduler.shutdown().await.expect("shutdown");
}

/// A kept place binds only the files after it: the file ahead of it starts, being the first in
/// list order that may run, and the one behind it waits even though a second place is free.
#[tokio::test]
async fn a_kept_place_binds_only_the_files_after_it() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = queue(directory.path(), 2).await;
    let files = packages(
        &scheduler,
        directory.path(),
        &["ahead", "returning", "behind"],
    )
    .await;
    let now = Utc::now();
    waits(
        &database,
        &files[1],
        FailureKind::Offline,
        now + chrono::Duration::seconds(8),
    )
    .await;

    pass(&scheduler, now).await;
    assert_eq!(running(&scheduler).await, HashSet::from([files[0].id]));

    pass(&scheduler, now + chrono::Duration::seconds(9)).await;
    assert_eq!(
        running(&scheduler).await,
        HashSet::from([files[0].id, files[1].id])
    );
    scheduler.shutdown().await.expect("shutdown");
}
