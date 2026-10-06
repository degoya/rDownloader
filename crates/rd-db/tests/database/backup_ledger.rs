//! The full backup's destinations, archive ledger, per-destination history and verifications
//! (RD-160-02, migration 0110).

use chrono::{Duration, Utc};
use rd_core::{BackupOrigin, BackupRunState, BackupVerifyState};
use rd_db::{
    BackupRunDestinationEnd, BackupVerification, BackupVerificationOutcome, Database,
    NewBackupArchive, NewBackupDestination, NewBackupRun,
};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("ledger.sqlite3"))
        .await
        .expect("database")
}

fn local(path: &str, keep_last: Option<u32>) -> NewBackupDestination {
    NewBackupDestination {
        kind: "local".to_owned(),
        name: path.to_owned(),
        config: serde_json::json!({ "path": path }),
        enabled: true,
        keep_last,
        keep_days: None,
    }
}

fn archive(destination_id: &str, run: &str, name: &str, days_ago: i64) -> NewBackupArchive {
    NewBackupArchive {
        destination_id: destination_id.to_owned(),
        run_id: run.to_owned(),
        archive_name: name.to_owned(),
        location: format!("/mnt/nas/{name}"),
        size_bytes: 4096,
        sha256: "ab".repeat(32),
        created_at: Utc::now() - Duration::days(days_ago),
    }
}

#[tokio::test]
async fn an_installation_has_an_id_and_any_number_of_destinations() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let config = database.backup_config().await.expect("config");
    assert_eq!(config.instance_id.len(), 8);
    assert!(
        config
            .instance_id
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );

    let nas = database
        .create_backup_destination(local("/mnt/nas", Some(7)))
        .await
        .expect("create");
    let cloud = database
        .create_backup_destination(NewBackupDestination {
            kind: "object_storage".to_owned(),
            name: "Bucket".to_owned(),
            config: serde_json::json!({ "profile_id": "1", "prefix": "bucket/rd" }),
            enabled: true,
            keep_last: None,
            keep_days: Some(30),
        })
        .await
        .expect("create");
    let listed = database.backup_config().await.expect("config").destinations;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, nas);
    assert_eq!(listed[0].keep_last, Some(7));
    assert_eq!(listed[1].keep_days, Some(30));

    assert!(
        database
            .update_backup_destination(nas.clone(), local("/mnt/other", None))
            .await
            .expect("update")
    );
    let updated = database
        .backup_destination(&nas)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(updated.config["path"], "/mnt/other");
    assert_eq!(updated.keep_last, None);
    assert!(
        !database
            .update_backup_destination("missing".to_owned(), local("/x", None))
            .await
            .expect("update")
    );
    assert!(
        database
            .delete_backup_destination(cloud)
            .await
            .expect("delete")
    );
    assert_eq!(database.backup_destinations().await.expect("list").len(), 1);
}

#[tokio::test]
async fn the_ledger_is_per_destination_and_goes_with_it() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let nas = database
        .create_backup_destination(local("/mnt/nas", Some(2)))
        .await
        .expect("create");
    let other = database
        .create_backup_destination(local("/mnt/other", None))
        .await
        .expect("create");
    let old = database
        .record_backup_archive(archive(&nas, "r1", "old.rdbackup", 3))
        .await
        .expect("record");
    database
        .record_backup_archive(archive(&nas, "r2", "new.rdbackup", 0))
        .await
        .expect("record");
    database
        .record_backup_archive(archive(&other, "r2", "new.rdbackup", 0))
        .await
        .expect("record");
    let at_nas = database.backup_archives(Some(&nas)).await.expect("ledger");
    assert_eq!(
        at_nas
            .iter()
            .map(|archive| archive.archive_name.as_str())
            .collect::<Vec<_>>(),
        ["new.rdbackup", "old.rdbackup"]
    );
    assert_eq!(database.backup_archives(None).await.expect("all").len(), 3);

    // The same name at the same destination is one archive, recorded again.
    database
        .record_backup_archive(archive(&nas, "r3", "new.rdbackup", 0))
        .await
        .expect("again");
    assert_eq!(
        database
            .backup_archives(Some(&nas))
            .await
            .expect("ledger")
            .len(),
        2
    );

    assert_eq!(
        database
            .forget_backup_archives(vec![old.clone()])
            .await
            .expect("forget"),
        1
    );
    assert!(database.backup_archive(&old).await.expect("read").is_none());

    // Removing a destination forgets its archives, not the other destination's.
    database
        .delete_backup_destination(nas.clone())
        .await
        .expect("delete");
    assert!(
        database
            .backup_archives(Some(&nas))
            .await
            .expect("ledger")
            .is_empty()
    );
    assert_eq!(
        database
            .backup_archives(Some(&other))
            .await
            .expect("ledger")
            .len(),
        1
    );
}

#[tokio::test]
async fn each_destination_of_a_run_has_its_row_and_a_stop_interrupts_them() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let run = |id: &str| NewBackupRun {
        id: id.to_owned(),
        origin: BackupOrigin::Manual,
        started_at: Utc::now(),
        destination_id: None,
        destination: None,
    };
    assert!(database.begin_backup_run(run("a")).await.expect("begin"));
    database
        .begin_backup_run_destinations(
            "a".to_owned(),
            vec![
                ("d1".to_owned(), "local".to_owned(), "/mnt/nas".to_owned()),
                ("d2".to_owned(), "rclone".to_owned(), "webdav:rd".to_owned()),
            ],
        )
        .await
        .expect("destinations");
    database
        .finish_backup_run_destination(BackupRunDestinationEnd {
            run_id: "a".to_owned(),
            destination_id: "d1".to_owned(),
            state: BackupRunState::Succeeded,
            attempts: 1,
            location: Some("/mnt/nas/a.rdbackup".to_owned()),
            pruned: 2,
            error_code: None,
            error_detail: None,
        })
        .await
        .expect("finish");
    let row = database.backup_run("a").await.expect("read").expect("row");
    assert_eq!(row.destinations.len(), 2);
    assert_eq!(row.destinations[0].state, BackupRunState::Succeeded);
    assert_eq!(row.destinations[0].pruned, 2);
    assert_eq!(row.destinations[1].state, BackupRunState::Running);

    // The process stops while the rclone remote is still being written.
    assert_eq!(
        database.interrupt_backup_runs().await.expect("interrupt"),
        1
    );
    let row = database.backup_run("a").await.expect("read").expect("row");
    assert_eq!(row.destinations[0].state, BackupRunState::Succeeded);
    assert_eq!(row.destinations[1].state, BackupRunState::Interrupted);
    assert_eq!(
        row.destinations[1].error_code.as_deref(),
        Some(rd_db::BACKUP_INTERRUPTED)
    );
}

#[tokio::test]
async fn a_verification_lands_in_the_history_and_on_its_archive() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let nas = database
        .create_backup_destination(local("/mnt/nas", None))
        .await
        .expect("create");
    let archive_id = database
        .record_backup_archive(archive(&nas, "r1", "a.rdbackup", 1))
        .await
        .expect("record");
    let started = |id: &str| BackupVerification {
        id: id.to_owned(),
        origin: BackupOrigin::Scheduled,
        state: BackupVerifyState::Running,
        archive_id: Some(archive_id.clone()),
        destination_id: Some(nas.clone()),
        destination: "/mnt/nas".to_owned(),
        archive_name: "a.rdbackup".to_owned(),
        started_at: Utc::now(),
        finished_at: None,
        content_checked: None,
        error_code: None,
        error_detail: None,
    };
    database
        .begin_backup_verification(started("v1"))
        .await
        .expect("begin");
    database
        .finish_backup_verification(
            "v1".to_owned(),
            BackupVerificationOutcome::Failed {
                code: "backup.verify_digest_mismatch".to_owned(),
                detail: "changed".to_owned(),
            },
        )
        .await
        .expect("finish");
    let checked = database
        .backup_archive(&archive_id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(checked.verify_state, Some(BackupVerifyState::Failed));
    assert_eq!(
        checked.verify_code.as_deref(),
        Some("backup.verify_digest_mismatch")
    );

    database
        .begin_backup_verification(started("v2"))
        .await
        .expect("begin");
    assert_eq!(
        database.interrupt_backup_runs().await.expect("interrupt"),
        0
    );
    let history = database.backup_verifications(10).await.expect("history");
    assert_eq!(history.len(), 2);
    let interrupted = history.iter().find(|row| row.id == "v2").expect("v2");
    assert_eq!(interrupted.state, BackupVerifyState::Interrupted);
    let failed = history.iter().find(|row| row.id == "v1").expect("v1");
    assert_eq!(failed.state, BackupVerifyState::Failed);

    let due = Utc::now() + Duration::days(7);
    database.arm_backup_verify(Some(due)).await.expect("arm");
    assert_eq!(
        database
            .backup_config()
            .await
            .expect("config")
            .verify_next_run_at
            .map(|at| at.timestamp()),
        Some(due.timestamp())
    );
}
