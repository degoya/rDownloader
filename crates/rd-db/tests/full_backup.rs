//! The full backup's store (RD-160-01, migration 0109): the consistent snapshot while writes
//! keep arriving, what is read out of it, and the configuration and run history.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{Duration, Utc};
use rd_core::{BackupOrigin, BackupRunState, ChunkId, DownloadId};
use rd_db::{
    BACKUP_INTERRUPTED, BackupConfigUpdate, BackupKeyRecord, BackupRunOutcome, Database,
    NewBackupRun, NewPluginRepository, NewPluginTrustedKey, NewPluginVersionChoice, PersistedChunk,
    PluginRepositoryInstall, PluginWithdrawnKey,
};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("backup.sqlite3"))
        .await
        .expect("database")
}

/// Keys of the `settings` table named `probe-<n>`, as numbers.
async fn probe_numbers(path: &std::path::Path) -> Vec<u64> {
    let snapshot = rd_db::snapshot::read_tables(path, &["settings"])
        .await
        .expect("read snapshot");
    let mut numbers: Vec<u64> = snapshot["settings"]
        .iter()
        .filter_map(|row| row["key"].as_str()?.strip_prefix("probe-")?.parse().ok())
        .collect();
    numbers.sort_unstable();
    numbers
}

#[tokio::test]
async fn the_snapshot_is_one_point_in_the_writer_s_order_while_writes_keep_arriving() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    // A writer that never stops while the snapshot is taken, like a running download's
    // checkpoints. Every write is acknowledged before the next is sent, so the acknowledged
    // count is a lower bound of what any later snapshot must hold.
    let acknowledged = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let database = database.clone();
        let acknowledged = acknowledged.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut next = 0_u64;
            while !stop.load(Ordering::SeqCst) {
                database
                    .set_setting(format!("probe-{next}"), serde_json::json!(next))
                    .await
                    .expect("write during the snapshot");
                next += 1;
                acknowledged.store(next, Ordering::SeqCst);
            }
            next
        })
    };
    while acknowledged.load(Ordering::SeqCst) < 50 {
        tokio::task::yield_now().await;
    }
    let before = acknowledged.load(Ordering::SeqCst);
    let path = directory.path().join("snapshot.sqlite3");
    database.snapshot_into(&path).await.expect("snapshot");
    let after = acknowledged.load(Ordering::SeqCst);
    // Downloads keep running: the writer goes on after the snapshot.
    while acknowledged.load(Ordering::SeqCst) < after + 50 {
        tokio::task::yield_now().await;
    }
    stop.store(true, Ordering::SeqCst);
    let written = writer.await.expect("writer task");

    let numbers = probe_numbers(&path).await;
    let held = u64::try_from(numbers.len()).expect("count");
    // Exactly the first `held` writes and none of the later ones: a prefix, never a gap.
    assert_eq!(numbers, (0..held).collect::<Vec<_>>());
    assert!(
        held >= before,
        "the snapshot lost acknowledged writes: {held} < {before}"
    );
    assert!(
        held <= after + 1,
        "the snapshot holds writes sent after it: {held} > {after}"
    );
    assert!(written > held, "the writer stopped at the snapshot");

    // The copy is a database in its own right, passes SQLite's own check and opens as one.
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&path)
        .read_only(true);
    let mut connection = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&options)
        .await
        .expect("open the copy");
    let verdict: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut connection)
        .await
        .expect("integrity check");
    assert_eq!(verdict, "ok");
    drop(connection);
    Database::open(&path).await.expect("the snapshot opens");
}

#[tokio::test]
async fn a_snapshot_never_overwrites_a_file_that_is_there() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let path = directory.path().join("taken.sqlite3");
    std::fs::write(&path, b"something else").expect("occupy");
    assert!(database.snapshot_into(&path).await.is_err());
    assert_eq!(std::fs::read(&path).expect("read"), b"something else");
}

#[tokio::test]
async fn the_plugin_trust_is_read_from_the_snapshot_table_by_table() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    database
        .trust_plugin_key(NewPluginTrustedKey {
            key_id: "author-v1".to_owned(),
            public_key: "AAAA".to_owned(),
            fingerprint: "cd".repeat(32),
            plugin_name: Some("Example".to_owned()),
        })
        .await
        .expect("trust");
    database
        .add_plugin_repository(NewPluginRepository {
            id: "community".to_owned(),
            name: "Community".to_owned(),
            url: "https://plugins.example.org/index.json".to_owned(),
            key_id: "community-v1".to_owned(),
            public_key: "BBBB".to_owned(),
            fingerprint: "ab".repeat(32),
        })
        .await
        .expect("repository");
    database
        .withdraw_plugin_key(PluginWithdrawnKey {
            fingerprint: "ef".repeat(32),
            key_id: "old-v1".to_owned(),
            repository_id: "community".to_owned(),
            withdrawn_at: "2026-09-01T00:00:00Z".to_owned(),
        })
        .await
        .expect("withdraw");
    database
        .record_plugin_repository_install(PluginRepositoryInstall {
            plugin_id: "example".to_owned(),
            version: "1.0.0".to_owned(),
            digest: "01".repeat(32),
            repository_id: "community".to_owned(),
            installed_at: "2026-09-02T00:00:00Z".to_owned(),
        })
        .await
        .expect("install");
    database
        .save_plugin_version_choice(NewPluginVersionChoice {
            plugin_id: "example".to_owned(),
            active_version: Some("1.0.0".to_owned()),
            previous_version: None,
            staged_version: None,
            update_policy: "automatic".to_owned(),
        })
        .await
        .expect("choice");
    let path = directory.path().join("trust.sqlite3");
    database.snapshot_into(&path).await.expect("snapshot");

    let trust = rd_db::snapshot::read_tables(&path, rd_db::snapshot::PLUGIN_TRUST_TABLES)
        .await
        .expect("trust");
    assert_eq!(trust.len(), rd_db::snapshot::PLUGIN_TRUST_TABLES.len());
    assert_eq!(trust["plugin_trusted_keys"][0]["key_id"], "author-v1");
    // The official repository is seeded by the migration; the community one follows it.
    assert_eq!(trust["plugin_repositories"].len(), 2);
    assert_eq!(
        trust["plugin_repositories"][1]["fingerprint"],
        "ab".repeat(32)
    );
    assert_eq!(trust["plugin_withdrawn_keys"][0]["key_id"], "old-v1");
    assert_eq!(
        trust["plugin_repository_installs"][0]["digest"],
        "01".repeat(32)
    );
    assert_eq!(
        trust["plugin_version_choices"][0]["update_policy"],
        "automatic"
    );
    assert!(trust["plugin_digest_revocations"].is_empty());
}

#[tokio::test]
async fn unfinished_downloads_are_listed_with_their_checkpoints() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "partial".to_owned(),
            destination: directory.path().join("downloads").display().to_string(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let id = DownloadId::new();
    database
        .create_download(rd_db::NewDownload {
            id,
            package_id,
            source: url::Url::parse("https://files.example.org/big.iso").expect("url"),
            file_name: "big.iso".to_owned(),
            total_bytes: Some(rd_core::ByteCount::new(32 * 1024).expect("bytes")),
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    let chunk = ChunkId::new();
    database
        .prepare_transfer(
            id,
            Some(32 * 1024),
            None,
            None,
            vec![PersistedChunk {
                id: chunk,
                start: 0,
                end: Some(32 * 1024),
                committed: 0,
            }],
        )
        .await
        .expect("plan");
    database
        .checkpoint_chunk(chunk, 8 * 1024)
        .await
        .expect("checkpoint");
    let path = directory.path().join("partial.sqlite3");
    database.snapshot_into(&path).await.expect("snapshot");

    let partial = rd_db::snapshot::read_partial_transfers(&path)
        .await
        .expect("partial");
    assert_eq!(partial.len(), 1);
    assert_eq!(partial[0]["id"], id.to_string());
    assert_eq!(partial[0]["file_name"], "big.iso");
    assert_eq!(partial[0]["chunks"][0]["committed"], 8 * 1024);
}

#[tokio::test]
async fn the_configuration_keeps_its_schedule_and_hands_back_a_replaced_key() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    let initial = database.backup_config().await.expect("config");
    assert!(!initial.enabled);
    assert_eq!(initial.schedule, "0 3 * * *");
    assert!(initial.key.is_none());
    assert!(initial.destinations.is_empty());
    assert!(initial.verify_schedule.is_none());

    let due = Utc::now() + Duration::hours(3);
    database
        .save_backup_config(BackupConfigUpdate {
            enabled: true,
            schedule: "30 2 * * *".to_owned(),
            timezone: "Europe/Berlin".to_owned(),
            next_run_at: Some(due),
            verify_schedule: Some("0 5 * * 0".to_owned()),
            verify_next_run_at: Some(due + Duration::days(1)),
        })
        .await
        .expect("save");
    let saved = database.backup_config().await.expect("config");
    assert_eq!(saved.timezone, "Europe/Berlin");
    assert_eq!(
        saved.next_run_at.map(|at| at.timestamp()),
        Some(due.timestamp())
    );
    assert_eq!(saved.verify_schedule.as_deref(), Some("0 5 * * 0"));

    let first = BackupKeyRecord {
        reference: "vault://first".to_owned(),
        salt: "c2FsdA==".to_owned(),
        fingerprint: "0011".to_owned(),
        set_at: Utc::now(),
    };
    assert_eq!(database.set_backup_key(first).await.expect("key"), None);
    let second = BackupKeyRecord {
        reference: "vault://second".to_owned(),
        salt: "c2FsdDI=".to_owned(),
        fingerprint: "2233".to_owned(),
        set_at: Utc::now(),
    };
    assert_eq!(
        database.set_backup_key(second).await.expect("key"),
        Some("vault://first".to_owned())
    );
    let keyed = database.backup_config().await.expect("config");
    assert_eq!(keyed.key.expect("key").reference, "vault://second");

    // A restart reads the same due time back instead of computing a new one.
    drop(database);
    let reopened = Database::open(directory.path().join("backup.sqlite3"))
        .await
        .expect("reopen");
    let restarted = reopened.backup_config().await.expect("config");
    assert_eq!(
        restarted.next_run_at.map(|at| at.timestamp()),
        Some(due.timestamp())
    );
}

fn run(id: &str) -> NewBackupRun {
    NewBackupRun {
        id: id.to_owned(),
        origin: BackupOrigin::Manual,
        started_at: Utc::now(),
        destination_id: None,
        destination: Some("/backups".to_owned()),
    }
}

#[tokio::test]
async fn one_run_at_a_time_and_the_history_says_how_each_ended() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    assert!(database.begin_backup_run(run("a")).await.expect("begin"));
    assert!(!database.begin_backup_run(run("b")).await.expect("second"));
    database
        .finish_backup_run(
            "a".to_owned(),
            BackupRunOutcome::Failed {
                code: "backup.destination_failed".to_owned(),
                detail: "disk full".to_owned(),
            },
        )
        .await
        .expect("finish");
    assert!(database.begin_backup_run(run("c")).await.expect("begin"));
    database
        .finish_backup_run(
            "c".to_owned(),
            BackupRunOutcome::Succeeded {
                archive_name: "rdownloader-backup.rdbackup".to_owned(),
                size_bytes: 4096,
                sha256: "aa".repeat(32),
                parts: serde_json::json!([{ "name": "database.sqlite3" }]),
                error_code: None,
                error_detail: None,
            },
        )
        .await
        .expect("finish");
    let failed = database.backup_run("a").await.expect("read").expect("row");
    assert_eq!(failed.state, BackupRunState::Failed);
    assert_eq!(
        failed.error_code.as_deref(),
        Some("backup.destination_failed")
    );
    let succeeded = database.backup_run("c").await.expect("read").expect("row");
    assert_eq!(succeeded.state, BackupRunState::Succeeded);
    assert_eq!(succeeded.size_bytes, Some(4096));
    assert_eq!(database.backup_runs(10).await.expect("list").len(), 2);
}

#[tokio::test]
async fn a_run_still_running_at_the_start_is_recorded_as_interrupted() {
    let directory = TempDir::new().expect("temp");
    let database = database(&directory).await;
    assert!(
        database
            .begin_backup_run(run("stopped"))
            .await
            .expect("begin")
    );
    assert_eq!(
        database.interrupt_backup_runs().await.expect("interrupt"),
        1
    );
    let row = database
        .backup_run("stopped")
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.state, BackupRunState::Interrupted);
    assert_eq!(row.error_code.as_deref(), Some(BACKUP_INTERRUPTED));
    // The slot is free again.
    assert!(database.begin_backup_run(run("next")).await.expect("begin"));
}
