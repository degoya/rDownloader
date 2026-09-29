//! The ledger's two crash points (RD-170-07, recovery matrix): an archive at its destination
//! that the ledger has not recorded yet, and an archive retention removed there that the ledger
//! still lists.
#![cfg(feature = "failpoints")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rd_backup::ledger::{self, Delivered, Run, Target};
use rd_backup::{
    BackupKey, BackupSources, LocalFolder, RetryPolicy, SealedBackup, archive_name, seal_backup,
    staging_root,
};
use rd_core::{BackupOrigin, BackupRunState};
use rd_db::{
    BackupDestinationRecord, BackupRunOutcome, Database, NewBackupArchive, NewBackupDestination,
    NewBackupRun,
};
use tempfile::TempDir;

const QUICK: RetryPolicy = RetryPolicy {
    attempts: 1,
    first_delay: Duration::ZERO,
};

struct Installation {
    data: PathBuf,
    nas: PathBuf,
    database: Database,
    destination: BackupDestinationRecord,
    instance: String,
}

impl Installation {
    async fn new(directory: &Path, keep_last: Option<u32>) -> Self {
        let data = directory.join("data");
        let nas = directory.join("nas");
        let database = Database::open(data.join("rdownloader.sqlite3"))
            .await
            .expect("database");
        let id = database
            .create_backup_destination(NewBackupDestination {
                kind: LocalFolder::KIND.to_owned(),
                name: "NAS".to_owned(),
                config: LocalFolder::config_of(&nas),
                enabled: true,
                keep_last,
                keep_days: None,
            })
            .await
            .expect("destination");
        let destination = database
            .backup_destination(&id)
            .await
            .expect("read")
            .expect("row");
        let instance = database.backup_config().await.expect("config").instance_id;
        LocalFolder::open(&nas).await.expect("folder");
        Self {
            data,
            nas,
            database,
            destination,
            instance,
        }
    }

    /// The restart: the same database file opened again, and the recovery every start runs.
    async fn restart(self) -> Self {
        let Self {
            data,
            nas,
            database,
            destination,
            instance,
        } = self;
        drop(database);
        let database = Database::open(data.join("rdownloader.sqlite3"))
            .await
            .expect("reopen");
        database.interrupt_backup_runs().await.expect("recover");
        Self {
            data,
            nas,
            database,
            destination,
            instance,
        }
    }

    async fn folder(&self) -> LocalFolder {
        LocalFolder::open(&self.nas).await.expect("folder")
    }

    /// One scheduled run to the one destination, the way the service runs it.
    async fn run(
        &self,
        key: &BackupKey,
        id: &str,
        at: DateTime<Utc>,
    ) -> (SealedBackup, Vec<Delivered>) {
        assert!(
            self.database
                .begin_backup_run(NewBackupRun {
                    id: id.to_owned(),
                    origin: BackupOrigin::Scheduled,
                    started_at: at,
                    destination_id: None,
                    destination: None,
                })
                .await
                .expect("begin")
        );
        self.database
            .begin_backup_run_destinations(
                id.to_owned(),
                vec![(
                    self.destination.id.clone(),
                    self.destination.kind.clone(),
                    ledger::label(&self.destination),
                )],
            )
            .await
            .expect("destinations");
        let sealed = seal_backup(
            &self.database,
            BackupSources {
                settings_bundle: b"{}".to_vec(),
                torrent_session: None,
                torrent_files: None,
                app_version: "test".to_owned(),
                instance_id: self.instance.clone(),
            },
            key,
            &staging_root(&self.data),
            id,
            at,
        )
        .await
        .expect("seal");
        let targets = vec![Target {
            record: self.destination.clone(),
            opened: Ok(Box::new(self.folder().await)),
        }];
        let delivered = ledger::deliver(
            &self.database,
            Run {
                id,
                instance_id: &self.instance,
                sealed: &sealed,
                created_at: at,
            },
            targets,
            QUICK,
        )
        .await;
        // A run whose delivery came back is finished the way the service finishes it; a run the
        // crash point stopped (no outcome) stays `running`, as it would after a kill.
        if delivered.iter().any(|done| done.result.is_ok()) {
            self.database
                .finish_backup_run(
                    id.to_owned(),
                    BackupRunOutcome::Succeeded {
                        archive_name: sealed.archive_name.clone(),
                        size_bytes: sealed.size_bytes,
                        sha256: sealed.sha256.clone(),
                        parts: serde_json::Value::Null,
                        error_code: None,
                        error_detail: None,
                    },
                )
                .await
                .expect("finish");
        }
        (sealed, delivered)
    }

    fn archives_at_destination(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.nas)
            .expect("nas")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    async fn ledger(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .database
            .backup_archives(Some(&self.destination.id))
            .await
            .expect("ledger")
            .into_iter()
            .map(|archive| archive.archive_name)
            .collect();
        names.sort();
        names
    }
}

async fn key() -> BackupKey {
    BackupKey::derive_new("correct horse battery")
        .await
        .expect("key")
}

#[tokio::test]
async fn an_archive_the_ledger_never_recorded_stays_whole_and_retention_leaves_it() {
    let directory = TempDir::new().expect("temp");
    let installation = Installation::new(directory.path(), Some(1)).await;
    let key = key().await;
    let now = Utc::now();

    let (first, delivered) = installation
        .run(&key, "first", now - chrono::Duration::hours(2))
        .await;
    assert!(delivered[0].result.is_ok());
    let crashed = {
        let guard = rd_core::failpoint::FailpointGuard::once("backup.before_archive_recorded");
        let (sealed, delivered) = installation
            .run(&key, "crashed", now - chrono::Duration::hours(1))
            .await;
        assert!(guard.fired(), "the crash point was never reached");
        assert!(
            delivered.is_empty(),
            "the stopped delivery reported an outcome"
        );
        sealed
    };
    // The archive is at the destination, whole; the ledger does not know it; the row runs on.
    let at_destination = installation.nas.join(&crashed.archive_name);
    let (size, sha256) = rd_backup::archive::digest_file(&at_destination).expect("digest");
    assert_eq!((size, sha256), (crashed.size_bytes, crashed.sha256.clone()));
    assert_eq!(
        installation.ledger().await,
        vec![first.archive_name.clone()]
    );
    let row = installation
        .database
        .backup_run("crashed")
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.destinations[0].state, BackupRunState::Running);

    let installation = installation.restart().await;
    let row = installation
        .database
        .backup_run("crashed")
        .await
        .expect("read")
        .expect("row");
    assert_eq!(row.state, BackupRunState::Interrupted);
    assert_eq!(row.destinations[0].state, BackupRunState::Interrupted);

    // The next run keeps one archive of the ledger's; the unrecorded one is not the ledger's.
    let (next, delivered) = installation.run(&key, "next", now).await;
    assert!(delivered[0].result.is_ok());
    assert_eq!(installation.ledger().await, vec![next.archive_name.clone()]);
    let mut expected = vec![crashed.archive_name, next.archive_name];
    expected.sort();
    assert_eq!(installation.archives_at_destination(), expected);
}

#[tokio::test]
async fn an_archive_removed_before_the_ledger_forgot_it_is_forgotten_by_the_next_pass() {
    let directory = TempDir::new().expect("temp");
    let installation = Installation::new(directory.path(), Some(1)).await;
    let now = Utc::now();
    // Three archives of this installation, oldest first, at the destination and in the ledger.
    let names: Vec<String> = (1..=3)
        .rev()
        .map(|hours| archive_name(&installation.instance, now - chrono::Duration::hours(hours)))
        .collect();
    for (index, name) in names.iter().enumerate() {
        std::fs::write(installation.nas.join(name), b"sealed").expect("archive");
        installation
            .database
            .record_backup_archive(NewBackupArchive {
                destination_id: installation.destination.id.clone(),
                run_id: format!("r{index}"),
                archive_name: name.clone(),
                location: installation.nas.join(name).display().to_string(),
                size_bytes: 6,
                sha256: "ab".repeat(32),
                created_at: now - chrono::Duration::hours(3 - i64::try_from(index).expect("index")),
            })
            .await
            .expect("record");
    }

    {
        let guard = rd_core::failpoint::FailpointGuard::once("backup.after_retention_removal");
        let folder = installation.folder().await;
        let stopped = ledger::prune(
            &installation.database,
            &folder,
            &installation.destination,
            &installation.instance,
        )
        .await;
        assert!(stopped.is_err());
        assert!(guard.fired(), "the crash point was never reached");
    }
    // One archive is gone at the destination; the ledger still lists all three, so it never
    // lists fewer than the destination holds.
    let left = installation.archives_at_destination();
    assert_eq!(left.len(), 2);
    assert!(left.contains(&names[2]), "the newest archive was removed");
    assert_eq!(installation.ledger().await, names);

    let installation = installation.restart().await;
    let folder = installation.folder().await;
    let pruned = ledger::prune(
        &installation.database,
        &folder,
        &installation.destination,
        &installation.instance,
    )
    .await
    .expect("prune");
    assert_eq!(pruned, 2);
    assert_eq!(installation.ledger().await, vec![names[2].clone()]);
    assert_eq!(
        installation.archives_at_destination(),
        vec![names[2].clone()]
    );
}
