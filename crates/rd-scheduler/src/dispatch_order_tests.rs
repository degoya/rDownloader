//! RD-1410-01 -- package by package through the real dispatch pass, on a fixed clock.
//!
//! The harness is RD-1250-07's (`dispatch_selection_tests.rs`): the supervisor's own passes are
//! held, each case drives the pass with a `now` of its own, and a stand-in FTP runner holds every
//! file it is given until it is cancelled. Every case runs with `sequential_packages` on; the
//! RD-1250-07 cases there run with it off, as every installation does by default.

use std::{collections::HashSet, path::Path};

use chrono::{DateTime, Duration, Utc};
use rd_core::{DownloadFile, DownloadKind, DownloadPriority, Failure, FailureKind};

use crate::{
    FileSpec, PackageSpec, RuntimeSettings, SchedulerHandle,
    dispatch_selection_tests::{pass, queue, running, waits},
};

/// A queue of `parallel` places with the setting as given.
async fn one_by_one(
    directory: &Path,
    parallel: usize,
    on: bool,
) -> (SchedulerHandle, rd_db::Database) {
    let (scheduler, database) = queue(directory, parallel).await;
    switch(&scheduler, parallel, on).await;
    (scheduler, database)
}

async fn switch(scheduler: &SchedulerHandle, parallel: usize, on: bool) {
    scheduler
        .update_runtime_settings(RuntimeSettings {
            max_active_files: parallel,
            sequential_packages: on,
            ..RuntimeSettings::default()
        })
        .await
        .expect("settings");
}

/// One package named `name` of `count` files, appended to the list.
async fn package(
    scheduler: &SchedulerHandle,
    directory: &Path,
    name: &str,
    priority: DownloadPriority,
    count: usize,
) -> Vec<DownloadFile> {
    let spec = PackageSpec {
        name: name.to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority,
        password: None,
        start_paused: false,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = (1..=count)
        .map(|part| FileSpec {
            source: format!("ftp://files.example/{name}.part{part}.rar")
                .parse()
                .expect("url"),
            file_name: format!("{name}.part{part}.rar"),
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
        })
        .collect();
    let (_, created) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    created
}

/// A hoster's limit of an hour on `file`.
async fn limited(database: &rd_db::Database, file: &DownloadFile, now: DateTime<Utc>) {
    waits(
        database,
        file,
        FailureKind::RateLimited {
            retry_after_seconds: Some(3600),
        },
        now + Duration::hours(1),
    )
    .await;
}

fn normal() -> DownloadPriority {
    DownloadPriority::Normal
}

/// The tester's queue: one place, the first package's file waits an hour for its hoster's
/// limit. No file of the next package starts; with the setting off it does, as before.
#[tokio::test]
async fn a_file_waiting_an_hour_for_its_limit_holds_the_next_package() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = one_by_one(directory.path(), 1, true).await;
    let first = package(&scheduler, directory.path(), "first", normal(), 1).await;
    let next = package(&scheduler, directory.path(), "next", normal(), 1).await;
    let now = Utc::now();
    limited(&database, &first[0], now).await;

    pass(&scheduler, now).await;
    assert!(
        running(&scheduler).await.is_empty(),
        "a file of the next package started while the first one waits"
    );

    switch(&scheduler, 1, false).await;
    pass(&scheduler, now).await;
    assert_eq!(running(&scheduler).await, HashSet::from([next[0].id]));
    scheduler.shutdown().await.expect("shutdown");
}

/// Two places: the first package's only file starts and holds nothing any more, so the next
/// package takes the second place in the same pass -- no place stays empty for the order.
#[tokio::test]
async fn a_running_file_holds_nothing_and_the_next_package_takes_the_second_place() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, _database) = one_by_one(directory.path(), 2, true).await;
    let first = package(&scheduler, directory.path(), "first", normal(), 1).await;
    let next = package(&scheduler, directory.path(), "next", normal(), 2).await;

    pass(&scheduler, Utc::now()).await;
    assert_eq!(
        running(&scheduler).await,
        HashSet::from([first[0].id, next[0].id])
    );
    scheduler.shutdown().await.expect("shutdown");
}

/// Two places, and the first package has a file waiting for its limit beside a queued one: its
/// queued file starts, the next package's does not, and the second place stays free.
#[tokio::test]
async fn a_waiting_file_lets_its_own_package_start_and_holds_the_next() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = one_by_one(directory.path(), 2, true).await;
    let first = package(&scheduler, directory.path(), "first", normal(), 2).await;
    package(&scheduler, directory.path(), "next", normal(), 1).await;
    let now = Utc::now();
    limited(&database, &first[0], now).await;

    pass(&scheduler, now).await;
    assert_eq!(running(&scheduler).await, HashSet::from([first[1].id]));
    scheduler.shutdown().await.expect("shutdown");
}

/// A package whose "not before" lies ahead holds nothing: otherwise the whole list would stand
/// until its moment.
#[tokio::test]
async fn a_package_not_due_holds_nothing() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = one_by_one(directory.path(), 1, true).await;
    let later = package(&scheduler, directory.path(), "later", normal(), 1).await;
    let next = package(&scheduler, directory.path(), "next", normal(), 1).await;
    let now = Utc::now();
    database
        .set_package_start_after(later[0].package_id, Some(now + Duration::hours(2)))
        .await
        .expect("not before");

    pass(&scheduler, now).await;
    assert_eq!(running(&scheduler).await, HashSet::from([next[0].id]));
    scheduler.shutdown().await.expect("shutdown");
}

/// A package whose files are paused, or failed for good, holds nothing.
#[tokio::test]
async fn a_paused_or_failed_package_holds_nothing() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = one_by_one(directory.path(), 1, true).await;
    let paused = package(&scheduler, directory.path(), "paused", normal(), 1).await;
    let failed = package(&scheduler, directory.path(), "failed", normal(), 1).await;
    let next = package(&scheduler, directory.path(), "next", normal(), 1).await;
    scheduler.pause(paused[0].id).await.expect("pause");
    database
        .record_failure(
            failed[0].id,
            Failure::new(FailureKind::Offline, "gone"),
            None,
        )
        .await
        .expect("failure");

    pass(&scheduler, Utc::now()).await;
    assert_eq!(running(&scheduler).await, HashSet::from([next[0].id]));
    scheduler.shutdown().await.expect("shutdown");
}

/// A package of higher priority comes first in the list, so it goes first even though it was
/// added after one whose file waits for its limit: the earlier package holds only the packages
/// after it in the list.
#[tokio::test]
async fn a_package_of_higher_priority_goes_first() {
    let directory = tempfile::tempdir().expect("temp");
    let (scheduler, database) = one_by_one(directory.path(), 1, true).await;
    let earlier = package(&scheduler, directory.path(), "earlier", normal(), 1).await;
    let urgent = package(
        &scheduler,
        directory.path(),
        "urgent",
        DownloadPriority::High,
        1,
    )
    .await;
    let now = Utc::now();
    limited(&database, &earlier[0], now).await;

    pass(&scheduler, now).await;
    assert_eq!(running(&scheduler).await, HashSet::from([urgent[0].id]));
    scheduler.shutdown().await.expect("shutdown");
}
