//! Crash and restart around a package folder move — Axis A of the RD-140-04 recovery matrix.
//!
//! Renaming a package folder (RD-106-13) and moving one into another category are the same
//! operation with the same two phases: the database row is written first, and the data on disk
//! follows. That order is deliberate — a row that names a folder which does not exist yet is
//! recoverable, while data in a folder no row names is not — but it does leave a window, and
//! the window is what this file addresses.
//!
//! The point is not that a move can fail; anything can fail. The point is that a process which
//! stops *inside* that window leaves a package whose data is still all in one place and whose
//! outstanding move is still recorded, so the next pass finishes it. That is what
//! `previous_destination` is for, and it is only worth anything if stopping there is actually
//! survivable, which ordinary tests never check because they never stop.
//!
//! Unlike the byte-level cases in `crates/rd-http/tests/crash_restart.rs`, the invariants here
//! are about a directory: nothing of the package is lost, nothing is left behind in the old
//! folder, and the move is not forgotten while it is unfinished.

#![cfg(feature = "failpoints")]

use std::path::{Path, PathBuf};

use rd_core::{DownloadFile, failpoint::FailpointGuard};
use rd_scheduler::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

/// A scheduler over a temporary database, plus one paused package below `storage/`.
///
/// Paused on purpose: `relocate_package` leaves running files where they are, so a case that
/// wants to prove what a *finished* package survives must not race the supervisor.
async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("scheduler-test.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    (scheduler, database)
}

/// The package every case here enqueues: one paused file below `storage/`.
fn one_paused_file(directory: &Path) -> (PackageSpec, Vec<FileSpec>) {
    (
        PackageSpec {
            name: "Example Package".to_owned(),
            // The base the package folder is created under, not the folder itself.
            destination: directory.join("storage"),
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            password: None,
            start_paused: true,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        },
        vec![FileSpec {
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
        }],
    )
}

async fn paused_package(
    directory: &Path,
) -> (SchedulerHandle, rd_db::Database, DownloadFile, PathBuf) {
    let (scheduler, database) = scheduler_over(directory).await;
    let (spec, files) = one_paused_file(directory);
    let (_package, files) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    let file = files.into_iter().next().expect("one file");
    let destination = directory.join("storage").join("Example Package");
    (scheduler, database, file, destination)
}

#[tokio::test]
async fn a_crash_before_the_folder_moves_leaves_the_rename_outstanding_and_finishable() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, database, file, destination) = paused_package(temporary.path()).await;
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    tokio::fs::write(destination.join(&file.file_name), b"payload")
        .await
        .expect("payload");
    // Extraction output: real content with no download row, so it is only carried over by the
    // directory sweep. It is here because a half-done move is exactly where such a file gets
    // forgotten.
    tokio::fs::write(destination.join("notes.txt"), b"mine")
        .await
        .expect("extra file");

    // Phase one, the way the endpoint does it: the row names the new folder, the disk does not.
    let renamed = temporary.path().join("storage").join("New Name");
    database
        .rename_package_directory(
            file.package_id,
            "New Name".to_owned(),
            renamed.to_string_lossy().into_owned(),
        )
        .await
        .expect("rename")
        .expect("package");

    // Phase two, interrupted before it has moved anything at all.
    let guard = FailpointGuard::once("scheduler.before_package_move");
    assert!(
        scheduler.relocate_package(file.package_id).await.is_err(),
        "the crash point did not stop the move"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    // The state a restart finds: the row is ahead of the disk, and says so.
    assert!(
        destination.join(&file.file_name).exists(),
        "the payload vanished although nothing had moved yet"
    );
    assert!(!renamed.exists(), "the new folder appeared before the move");
    assert_eq!(
        database
            .package_previous_destination(file.package_id)
            .await
            .expect("previous destination")
            .as_deref(),
        Some(destination.to_string_lossy().as_ref()),
        "the unfinished move was forgotten, which is what makes it unrecoverable"
    );

    // The restart: the very same call, with nothing armed, has to finish the job.
    scheduler
        .relocate_package(file.package_id)
        .await
        .expect("the resumed move");

    assert_eq!(
        tokio::fs::read(renamed.join(&file.file_name))
            .await
            .expect("moved payload"),
        b"payload",
        "the payload did not reach the renamed folder"
    );
    assert_eq!(
        tokio::fs::read(renamed.join("notes.txt"))
            .await
            .expect("moved extra file"),
        b"mine",
        "what the database never knew about was left behind"
    );
    assert!(
        !destination.exists(),
        "the old folder survived the completed move"
    );
    assert_eq!(
        database
            .package_previous_destination(file.package_id)
            .await
            .expect("previous destination"),
        None,
        "the completed move is still recorded as outstanding"
    );
}

#[tokio::test]
async fn a_crash_after_the_promote_rename_adopts_the_file_instead_of_fetching_it_again() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, database, file, destination) = paused_package(temporary.path()).await;
    let staging = destination.join(".rdownloader");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    let payload = b"the whole file, fetched exactly once";
    let part_path = staging.join(format!("{}.part", file.id));
    tokio::fs::write(&part_path, payload).await.expect("part");
    let total = u64::try_from(payload.len()).expect("payload length");
    // What the transfer had confirmed when it finished: the adoption is decided on the
    // recorded progress against the length of the file that is lying there.
    database
        .set_download_progress(file.id, total, Some(total))
        .await
        .expect("progress");
    let file = database
        .get_download(file.id)
        .await
        .expect("download")
        .expect("download row");
    let final_path = destination.join(&file.file_name);

    // Phase one: the part file is verified and renamed, and the process stops before the row
    // is told about it.
    let guard = FailpointGuard::once("scheduler.before_promote");
    assert!(
        scheduler
            .promote_finished_part(&file, &part_path, &final_path)
            .await
            .is_err(),
        "the crash point did not stop the promotion"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    // The state a restart finds: the payload is where the user expects it, the part file the
    // ordinary resume would look for is gone, and the queue still calls the transfer unfinished.
    assert_eq!(
        tokio::fs::read(&final_path)
            .await
            .expect("promoted payload"),
        payload,
        "the rename did not happen before the crash point"
    );
    assert!(!part_path.exists(), "the part file survived its own rename");
    assert_ne!(
        database
            .get_download(file.id)
            .await
            .expect("download")
            .expect("download row")
            .state,
        rd_core::DownloadState::Completed,
        "the row was completed although the crash point fired before it"
    );

    // The restart: the next pass over this file has to recognise it and finish.
    assert!(
        scheduler
            .adopt_finished_file(&file, &destination, &staging, &part_path, Some(total))
            .await
            .expect("the adoption"),
        "the finished file in its final place was not adopted"
    );

    let completed = database
        .get_download(file.id)
        .await
        .expect("download")
        .expect("download row");
    assert_eq!(
        completed.state,
        rd_core::DownloadState::Completed,
        "the adopted file did not finish the download"
    );
    assert_eq!(
        tokio::fs::read(&final_path).await.expect("payload"),
        payload,
        "the adopted file was overwritten"
    );
    assert!(
        !destination.join("file (1).bin").exists(),
        "a second copy was filed beside the first, which is the defect this covers"
    );
    assert!(
        !staging.exists(),
        "the staging directory was left behind by the adoption"
    );
}

/// A crash between the package row and its first file leaves an empty package behind.
///
/// It is the one interruption `enqueue_package` cannot prevent: a download row needs a package
/// to belong to, so the package is always written first, and the future that would have rolled
/// it back is gone with the process. What is left reads in the queue and in the interface
/// exactly like a package that finished without downloading anything — there is no state that
/// says "still filling up" — and it can never be removed the ordinary way, because removing a
/// package is a side effect of removing its last file and it has none.
#[tokio::test]
async fn a_crash_after_the_package_row_leaves_no_empty_package_behind() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, database) = scheduler_over(temporary.path()).await;
    let (spec, files) = one_paused_file(temporary.path());

    let guard = FailpointGuard::once("scheduler.after_package_row");
    assert!(
        scheduler.enqueue_package(spec, files).await.is_err(),
        "the crash point did not stop the enqueue"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    // The state a restart finds. Asserted rather than assumed: a case whose crash point fired
    // after the rollback had already run would prove nothing about the recovery below.
    assert_eq!(
        database.list_packages().await.expect("packages").len(),
        1,
        "the crash point fired somewhere other than between the row and its first file"
    );
    assert!(
        database
            .list_downloads()
            .await
            .expect("downloads")
            .is_empty(),
        "a file was written although the crash point fires before the first one"
    );

    // The restart: `recover_interrupted` is the first thing that touches the database.
    database.recover_interrupted().await.expect("recovery");

    assert!(
        database.list_packages().await.expect("packages").is_empty(),
        "the empty package survived the restart, where nothing can ever remove it again"
    );

    // And a package that does have files is not swept up with it: the same call, on a whole
    // package, has to leave it alone.
    let (spec, files) = one_paused_file(temporary.path());
    scheduler
        .enqueue_package(spec, files)
        .await
        .expect("a second, uninterrupted enqueue");
    database.recover_interrupted().await.expect("recovery");
    assert_eq!(
        database.list_packages().await.expect("packages").len(),
        1,
        "recovery removed a package that still had its file"
    );
}

/// Every registered crash point for this crate has a case here.
///
/// Registering a point and never arming it would list an invariant nobody checks, which reads
/// as coverage and is not.
#[test]
fn every_scheduler_crash_point_is_exercised_by_a_case() {
    // Two sources, because one crash point sits on a `pub(crate)` path that no integration
    // test can reach: its case is a unit test beside the code it interrupts.
    let source = concat!(
        include_str!("crash_restart.rs"),
        include_str!("../src/mirror_fallback_tests.rs"),
    );
    for point in rd_core::failpoint::CRASH_POINTS
        .iter()
        .filter(|point| point.owner == "rd-scheduler")
    {
        assert!(
            source.contains(&format!("FailpointGuard::once(\"{}\")", point.name)),
            "{} is registered but no case in this file arms it",
            point.name
        );
    }
}
