//! `pre_update.before_copy_published` and `pre_update.before_archive_published` (RD-180-03,
//! recovery matrix): the preparation before an update stops after it wrote its copy or sealed
//! its archive and before either carries its name.
#![cfg(feature = "failpoints")]

use std::path::Path;

use chrono::Utc;
use rd_backup::{
    BackupKey, BackupSources,
    pre_update::{self, UpdatePlan},
};
use rd_core::BackupOrigin;
use rd_db::{Database, NewBackupRun};
use tempfile::TempDir;

fn plan(data: &Path) -> UpdatePlan<'_> {
    UpdatePlan {
        data_directory: data,
        from_version: "1.8.0-beta.1",
        target_version: "1.8.0-beta.2",
        at: Utc::now(),
    }
}

fn sources() -> BackupSources {
    BackupSources {
        settings_bundle: b"{}".to_vec(),
        torrent_session: None,
        torrent_files: None,
        app_version: "1.8.0-beta.1".to_owned(),
        instance_id: "0a1b2c3d".to_owned(),
    }
}

/// Files directly in the folder whose name ends with `suffix`.
fn named(folder: &Path, suffix: &str) -> usize {
    std::fs::read_dir(folder).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(suffix))
            .count()
    })
}

async fn live(data: &Path) -> Database {
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("database");
    database
        .begin_backup_run(NewBackupRun {
            id: "live".to_owned(),
            origin: BackupOrigin::Manual,
            started_at: Utc::now(),
            destination_id: None,
            destination: None,
        })
        .await
        .expect("begin");
    database
}

#[tokio::test]
async fn a_copy_stopped_before_its_check_never_carries_a_name_and_the_live_database_is_untouched() {
    let directory = TempDir::new().expect("temp");
    let data = directory.path().join("data");
    let database = live(&data).await;
    let folder = pre_update::directory(&data);
    {
        let guard = rd_core::failpoint::FailpointGuard::once("pre_update.before_copy_published");
        let stopped = pre_update::copy_database(&database, plan(&data)).await;
        assert!(stopped.is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    // Written, unchecked, under its working name only: nothing a rollback would trust.
    assert_eq!(named(&folder, ".sqlite3"), 0);
    assert_eq!(named(&folder, ".partial"), 1);

    // The restart: the live database opens as it was, and the sweep every start runs.
    drop(database);
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("the live database still opens");
    assert!(database.backup_run("live").await.expect("read").is_some());
    pre_update::sweep(&data).await.expect("sweep");
    assert_eq!(named(&folder, ".partial"), 0);

    // The next preparation writes one whole, checked copy.
    let copy = pre_update::copy_database(&database, plan(&data))
        .await
        .expect("copy");
    rd_db::snapshot::check_integrity(&copy.path)
        .await
        .expect("whole");
    assert_eq!(named(&folder, ".sqlite3"), 1);
}

#[tokio::test]
async fn an_archive_stopped_before_it_was_published_never_reaches_the_folder() {
    let directory = TempDir::new().expect("temp");
    let data = directory.path().join("data");
    let database = live(&data).await;
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let folder = pre_update::directory(&data);
    {
        let guard = rd_core::failpoint::FailpointGuard::once("pre_update.before_archive_published");
        let stopped = pre_update::seal_archive(&database, sources(), &key, plan(&data)).await;
        assert!(stopped.is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    assert_eq!(named(&folder, ".rdbackup"), 0);
    assert!(folder.join("staging").exists());

    drop(database);
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("reopen");
    pre_update::sweep(&data).await.expect("sweep");
    // The unencrypted copy the sealing staged is gone with the staging.
    assert!(!folder.join("staging").exists());

    let archive = pre_update::seal_archive(&database, sources(), &key, plan(&data))
        .await
        .expect("archive");
    rd_backup::archive::verify_archive(&archive.path, &key).expect("verifies");
    assert_eq!(named(&folder, ".rdbackup"), 1);
}
