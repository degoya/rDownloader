//! RD-1390-07 — a failed package reset whole: what it made goes, a stranger's file stays, and
//! nothing goes at all while a refusal stands.

use std::path::{Path, PathBuf};

use rd_core::{
    DownloadFile, DownloadKind, DownloadState, ExtractionResult, Failure, FailureKind, PackageState,
};

use super::PackageNotFailed;
use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

/// A scheduler over a new database below `directory`.
async fn scheduler_over(directory: &Path) -> SchedulerHandle {
    let database = rd_db::Database::open(directory.join("scheduler-test.sqlite3"))
        .await
        .expect("database");
    start(directory, database).await
}

/// A scheduler that admits no transfer, so a row the reset queued stays queued.
async fn start(directory: &Path, database: rd_db::Database) -> SchedulerHandle {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database,
        SchedulerConfig {
            max_active_files: 0,
            ..SchedulerConfig::for_directory(directory.join("downloads"))
        },
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    scheduler.storage_recovery_finished().await;
    scheduler
}

fn file_spec(name: &str, kind: DownloadKind) -> FileSpec {
    FileSpec {
        source: format!("https://example.invalid/{name}")
            .parse()
            .expect("url"),
        file_name: name.to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        kind,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
        source_set: None,
    }
}

/// One paused package of `files` below `storage/`, and its folder.
async fn package_of(
    scheduler: &SchedulerHandle,
    directory: &Path,
    files: Vec<FileSpec>,
) -> (Vec<DownloadFile>, PathBuf) {
    let (_package, files) = scheduler
        .enqueue_package(
            PackageSpec {
                name: "Broken Set".to_owned(),
                destination: directory.join("storage"),
                category_id: None,
                priority: rd_core::DownloadPriority::High,
                password: None,
                start_paused: true,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            },
            files,
        )
        .await
        .expect("enqueue");
    let folder = directory.join("storage").join("Broken Set");
    tokio::fs::create_dir_all(&folder).await.expect("folder");
    (files, folder)
}

async fn fail(scheduler: &SchedulerHandle, file: &DownloadFile) {
    scheduler
        .database
        .record_failure(
            file.id,
            Failure::coded(FailureKind::Offline, "http.not_found", "the file is gone"),
            None,
        )
        .await
        .expect("failure");
}

async fn complete(scheduler: &SchedulerHandle, file: &DownloadFile) {
    for step in [DownloadState::Downloading, DownloadState::Verifying] {
        scheduler
            .database
            .transition_download(file.id, step)
            .await
            .expect("on its way");
    }
    scheduler
        .database
        .complete_download(file.id, file.file_name.clone(), None)
        .await
        .expect("complete");
}

/// A failed package as a corrupt set leaves it: the first volume failed with its `.part`
/// behind, the second finished, the unpack put a file and a folder beside them and failed.
/// `theirs.txt` was in the folder before and belongs to nobody in the queue.
async fn failed_package(directory: &Path) -> (SchedulerHandle, Vec<DownloadFile>, PathBuf) {
    let scheduler = scheduler_over(directory).await;
    let (files, folder) = package_of(
        &scheduler,
        directory,
        vec![
            file_spec("set.part1.rar", DownloadKind::Http),
            file_spec("set.part2.rar", DownloadKind::Http),
        ],
    )
    .await;
    fail(&scheduler, &files[0]).await;
    complete(&scheduler, &files[1]).await;
    let staging = folder.join(".rdownloader");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::write(staging.join(format!("{}.part", files[0].id)), b"half")
        .await
        .expect("part");
    tokio::fs::write(folder.join("set.part2.rar"), b"volume")
        .await
        .expect("volume");
    tokio::fs::write(folder.join("theirs.txt"), b"someone else's")
        .await
        .expect("foreign file");
    tokio::fs::write(folder.join("film.mkv"), b"half a film")
        .await
        .expect("unpacked file");
    tokio::fs::create_dir_all(folder.join("Extras"))
        .await
        .expect("unpacked folder");
    tokio::fs::write(folder.join("Extras").join("extra.txt"), b"extra")
        .await
        .expect("unpacked extra");
    let package = files[0].package_id;
    scheduler
        .database
        .record_package_outputs(
            package,
            vec![
                folder.join("film.mkv").to_string_lossy().into_owned(),
                folder.join("Extras").to_string_lossy().into_owned(),
            ],
        )
        .await
        .expect("outputs");
    scheduler
        .database
        .set_package_extraction(package, Some(ExtractionResult::Failed))
        .await
        .expect("extraction");
    scheduler
        .database
        .set_package_state(package, PackageState::Failed, None, None, None)
        .await
        .expect("failed");
    (scheduler, files, folder)
}

#[tokio::test]
async fn reset_package_deletes_what_the_package_made_and_keeps_a_strangers_file() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, files, folder) = failed_package(temporary.path()).await;

    let reset = scheduler
        .reset_package(files[0].package_id, true)
        .await
        .expect("reset");

    assert_eq!(reset, 2);
    assert!(
        !folder.join("set.part2.rar").exists(),
        "the finished volume"
    );
    assert!(
        !folder
            .join(".rdownloader")
            .join(format!("{}.part", files[0].id))
            .exists(),
        "the failed volume's part"
    );
    assert!(!folder.join("film.mkv").exists(), "the unpacked file");
    assert!(!folder.join("Extras").exists(), "the unpacked folder");
    assert_eq!(
        tokio::fs::read(folder.join("theirs.txt"))
            .await
            .expect("the stranger's file stays"),
        b"someone else's"
    );
    for file in scheduler
        .database
        .downloads_for_package(files[0].package_id)
        .await
        .expect("rows")
    {
        assert_eq!(file.state, DownloadState::Queued, "{}", file.file_name);
        assert!(file.last_error.is_none());
        assert_eq!(file.committed_bytes.get(), 0);
    }
    let package = scheduler
        .database
        .get_package(files[0].package_id)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(package.state, PackageState::Queued);
    assert_eq!(package.extraction_result, None);
    assert_eq!(
        package.name, "Broken Set",
        "what the LinkGrabber chose stays"
    );
    assert_eq!(package.priority, rd_core::DownloadPriority::High);
    for file in &files {
        assert!(
            !scheduler.active.lock().await.held.contains_key(&file.id),
            "the holds went with the reset"
        );
    }
}

#[tokio::test]
async fn reset_package_keeps_the_unpacked_files_when_told_to() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, files, folder) = failed_package(temporary.path()).await;

    scheduler
        .reset_package(files[0].package_id, false)
        .await
        .expect("reset");

    assert!(!folder.join("set.part2.rar").exists(), "the download goes");
    assert!(folder.join("film.mkv").exists(), "the unpacked file stays");
    assert!(folder.join("Extras").join("extra.txt").exists());
}

/// Nothing foreign in the folder: it goes with the data, and after a restart the package waits
/// from the start under the names it had, with nothing left for them to collide with.
#[tokio::test]
async fn reset_package_removes_the_emptied_folder_and_survives_a_restart() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, files, folder) = failed_package(temporary.path()).await;
    tokio::fs::remove_file(folder.join("theirs.txt"))
        .await
        .expect("no stranger");

    scheduler
        .reset_package(files[0].package_id, true)
        .await
        .expect("reset");
    assert!(!folder.exists(), "the emptied package folder went");
    scheduler.shutdown().await.expect("shutdown");

    let restarted = start(temporary.path(), scheduler.database.clone()).await;
    let rows = restarted
        .database
        .downloads_for_package(files[0].package_id)
        .await
        .expect("rows");
    assert_eq!(
        rows.iter()
            .map(|row| (row.file_name.as_str(), row.state))
            .collect::<Vec<_>>(),
        vec![
            ("set.part1.rar", DownloadState::Queued),
            ("set.part2.rar", DownloadState::Queued),
        ]
    );
    restarted.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reset_package_refuses_a_package_without_a_failure_and_deletes_nothing() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let scheduler = scheduler_over(temporary.path()).await;
    let (files, folder) = package_of(
        &scheduler,
        temporary.path(),
        vec![file_spec("film.mkv", DownloadKind::Http)],
    )
    .await;
    complete(&scheduler, &files[0]).await;
    tokio::fs::write(folder.join("film.mkv"), b"film")
        .await
        .expect("payload");

    let refused = scheduler
        .reset_package(files[0].package_id, true)
        .await
        .expect_err("nothing failed");

    assert!(
        refused.downcast_ref::<PackageNotFailed>().is_some(),
        "{refused:#}"
    );
    assert!(
        folder.join("film.mkv").exists(),
        "a refused reset deletes nothing"
    );
}

#[tokio::test]
async fn reset_package_refuses_while_a_file_is_at_work() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, files, folder) = failed_package(temporary.path()).await;
    scheduler
        .database
        .reset_download(files[1].id)
        .await
        .expect("back to queued");
    for state in [DownloadState::Resolving, DownloadState::Downloading] {
        scheduler
            .database
            .transition_download(files[1].id, state)
            .await
            .expect("running");
    }

    let refused = scheduler
        .reset_package(files[0].package_id, true)
        .await
        .expect_err("a running file keeps the package");

    assert_eq!(
        rd_db::store_kind(&refused),
        Some(rd_db::StoreErrorKind::WrongState)
    );
    assert!(
        folder.join("film.mkv").exists(),
        "a refused reset deletes nothing"
    );
}

#[tokio::test]
async fn reset_package_refuses_a_usenet_file_without_its_nzb_before_deleting() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let scheduler = scheduler_over(temporary.path()).await;
    let (files, folder) = package_of(
        &scheduler,
        temporary.path(),
        vec![file_spec("set.part1.rar", DownloadKind::Usenet)],
    )
    .await;
    assert!(files[0].nzb_file_id.is_none(), "no NZB behind the row");
    fail(&scheduler, &files[0]).await;
    tokio::fs::write(folder.join("set.part1.rar"), b"volume")
        .await
        .expect("payload");

    let refused = scheduler
        .reset_package(files[0].package_id, true)
        .await
        .expect_err("nothing left to fetch it from");

    assert!(
        refused.downcast_ref::<super::NzbDropped>().is_some(),
        "{refused:#}"
    );
    assert!(folder.join("set.part1.rar").exists());
}

/// `scheduler.before_package_reset_rows`: the package's data is gone, its rows are not written
/// back yet. The rows keep what they said, the package stays failed, and the same reset after
/// the restart finishes it.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_crash_between_the_data_and_the_rows_leaves_a_reset_that_can_be_asked_again() {
    use rd_core::failpoint::FailpointGuard;

    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, files, folder) = failed_package(temporary.path()).await;
    let package = files[0].package_id;

    let guard = FailpointGuard::once("scheduler.before_package_reset_rows");
    assert!(
        scheduler.reset_package(package, true).await.is_err(),
        "the crash point did not stop the reset"
    );
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);
    scheduler.shutdown().await.expect("shutdown");
    // The data went first.
    assert!(!folder.join("set.part2.rar").exists());
    assert!(!folder.join("film.mkv").exists());
    assert!(
        folder.join("theirs.txt").exists(),
        "the stranger's file stays"
    );
    // The rows still say what they had: less data than they know, never data without a row.
    let database = scheduler.database.clone();
    let rows = database.downloads_for_package(package).await.expect("rows");
    assert_eq!(
        rows.iter().map(|row| row.state).collect::<Vec<_>>(),
        vec![DownloadState::Failed, DownloadState::Completed]
    );
    let stopped = database
        .get_package(package)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(
        stopped.state,
        PackageState::Failed,
        "still offered for a reset"
    );
    assert_eq!(
        database
            .package_reset_plan(package)
            .await
            .expect("plan")
            .unpack_outputs
            .len(),
        2,
        "the record of the unpacked entries survives"
    );

    let restarted = start(temporary.path(), database.clone()).await;
    assert_eq!(
        restarted
            .reset_package(package, true)
            .await
            .expect("the reset asked again"),
        2
    );
    let rows = database.downloads_for_package(package).await.expect("rows");
    assert!(rows.iter().all(|row| row.state == DownloadState::Queued));
    let package_row = database
        .get_package(package)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(package_row.state, PackageState::Queued);
    assert_eq!(package_row.extraction_result, None);
    assert!(folder.join("theirs.txt").exists());
    restarted.shutdown().await.expect("shutdown");
}
