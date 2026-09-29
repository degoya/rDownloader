//! `restore.after_live_set_aside` (RD-160-03, recovery matrix): the switch to a restored state
//! stops after the live database was set aside and before the restored one took its place.
#![cfg(feature = "failpoints")]

mod common;

use common::{PASSPHRASE, WINDOWS_ROOT, windows_archive};
use rd_backup::manifest::DATABASE_PART;
use rd_backup::restore::cutover::{self, Cutover, Layout, PendingRestore, Phase};
use rd_backup::restore::inspect::{key_for, unpack};
use rd_db::restore_copy;
use tempfile::TempDir;

async fn prepared(archive: &std::path::Path, into: &std::path::Path) {
    let key = key_for(archive, PASSPHRASE).await.expect("key");
    unpack(archive, key, into).await.expect("unpack");
    restore_copy::migrate_copy(&into.join(DATABASE_PART))
        .await
        .expect("migrate");
}

#[tokio::test]
async fn a_switch_stopped_between_its_renames_is_finished_by_the_next_start() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;

    // The live installation: a database in rollback-journal mode, so the database is the
    // first item the switch sets aside.
    let live = directory.path().join("live");
    let unpacked = directory.path().join("live-unpacked");
    prepared(&fixture.archive, &unpacked).await;
    std::fs::create_dir_all(&live).expect("live");
    std::fs::rename(
        unpacked.join(DATABASE_PART),
        live.join("rdownloader.sqlite3"),
    )
    .expect("live database");
    let layout = Layout::new(&live.join("rdownloader.sqlite3"));
    let database = live.join("rdownloader.sqlite3");

    let work = directory.path().join("work");
    prepared(&fixture.archive, &work).await;
    let pending = PendingRestore {
        phase: Phase::Staged,
        staged_at: chrono::Utc::now(),
        archive_name: "fixture.rdbackup".to_owned(),
        backup_created_at: chrono::Utc::now(),
        app_version: "1.6.0-windows".to_owned(),
        minted_secrets: Vec::new(),
    };
    cutover::stage(&layout, &work, &pending).expect("stage");

    {
        let guard = rd_core::failpoint::FailpointGuard::once("restore.after_live_set_aside");
        assert!(cutover::switch(&layout, &pending).is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    // The stop: no database where the start looks, the previous one aside, the restored one
    // still staged, and the marker still says staged.
    assert!(!database.exists());
    assert!(layout.previous().join("rdownloader.sqlite3").exists());
    assert!(layout.staged().join(DATABASE_PART).exists());
    assert_eq!(
        cutover::read_marker(&layout)
            .expect("marker")
            .map(|marker| marker.phase),
        Some(Phase::Staged)
    );

    // The restart finishes the switch from exactly there and opens the restored database.
    let outcome = cutover::apply_pending(&layout).expect("start");
    assert!(matches!(outcome, Cutover::Switched(_)), "{outcome:?}");
    let opened = rd_db::Database::open(&database).await.expect("open");
    assert_eq!(
        opened.list_storage_roots().await.expect("roots")[0].path,
        WINDOWS_ROOT
    );
    assert!(layout.previous().join("rdownloader.sqlite3").exists());
    cutover::finish(&layout).expect("finish");
    assert!(!layout.previous().exists());
    assert!(cutover::read_marker(&layout).expect("marker").is_none());
}
