//! `backup.before_archive_published` (RD-160-01, recovery matrix): the archive is finished in
//! staging and the process stops before its destination has it.
#![cfg(feature = "failpoints")]

use rd_backup::{
    BackupKey, BackupSources, LocalFolder, create_backup, staging_root, sweep_staging,
};
use rd_core::{BackupOrigin, BackupRunState};
use rd_db::{Database, NewBackupRun};
use tempfile::TempDir;

fn sources() -> BackupSources {
    BackupSources {
        settings_bundle: b"{}".to_vec(),
        torrent_session: None,
        torrent_files: None,
        app_version: "test".to_owned(),
        instance_id: "0a1b2c3d".to_owned(),
    }
}

#[tokio::test]
async fn an_archive_that_never_reached_its_destination_leaves_nothing_there_and_the_start_cleans_up()
 {
    let directory = TempDir::new().expect("temp");
    let data = directory.path().join("data");
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("database");
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let nas = directory.path().join("nas");
    let destination = LocalFolder::open(&nas).await.expect("destination");
    let staging = staging_root(&data);
    let run = |id: &str| NewBackupRun {
        id: id.to_owned(),
        origin: BackupOrigin::Scheduled,
        started_at: chrono::Utc::now(),
        destination_id: None,
        destination: Some(nas.display().to_string()),
    };

    assert!(
        database
            .begin_backup_run(run("crashed"))
            .await
            .expect("begin")
    );
    {
        let guard = rd_core::failpoint::FailpointGuard::once("backup.before_archive_published");
        let crashed = create_backup(
            &database,
            sources(),
            &key,
            &destination,
            &staging,
            "crashed",
            chrono::Utc::now(),
        )
        .await;
        assert!(crashed.is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    // The finished archive is in staging, and nothing is at the destination.
    assert!(staging.join("crashed.rdbackup").exists());
    assert_eq!(std::fs::read_dir(&nas).expect("nas").count(), 0);

    // The restart: the same database file opened again, and the recovery every start runs.
    drop(database);
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("reopen");
    assert_eq!(database.interrupt_backup_runs().await.expect("recover"), 1);
    sweep_staging(&staging).await.expect("sweep");
    let row = database
        .backup_run("crashed")
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.state, BackupRunState::Interrupted);
    assert_eq!(row.error_code.as_deref(), Some(rd_db::BACKUP_INTERRUPTED));
    assert!(!staging.exists());

    // The next run goes through beside it.
    assert!(database.begin_backup_run(run("next")).await.expect("begin"));
    let created = create_backup(
        &database,
        sources(),
        &key,
        &destination,
        &staging,
        "next",
        chrono::Utc::now(),
    )
    .await
    .expect("next run");
    assert!(std::path::Path::new(&created.location).exists());
    assert_eq!(std::fs::read_dir(&nas).expect("nas").count(), 1);
}
