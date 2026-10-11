//! Opening the database: the connection options, the migrations and the writer actor; the
//! vault, and the maintenance that keeps or closes the files.

use std::{
    path::Path,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result};
use sqlx::{
    ConnectOptions, Connection, SqliteConnection,
    sqlite::{
        SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions,
        SqliteSynchronous,
    },
};
use tokio::sync::mpsc;
use tracing::log::LevelFilter;

use crate::{
    ConfigReplacement, Database, EventBus, MIGRATOR,
    commands::{MaintenanceCommand, WriterCommand},
    pre_migration,
    writer::{self, Writer},
};

impl Database {
    /// Opens a database file, applies migrations and starts the writer actor.
    ///
    /// An existing database with pending migrations is copied to `<data>/pre-migration/` first,
    /// and put back from that copy when a migration fails ([`pre_migration`]); the start then
    /// fails with [`pre_migration::MigrationFailure`].
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path.as_ref(), &MIGRATOR).await
    }

    /// [`Database::open`] with the migrations given; a test hands in a chain with one that fails.
    pub(crate) async fn open_with(path: &Path, migrator: &sqlx::migrate::Migrator) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            // What a row lets go of is overwritten with zeros rather than left in a free page
            // of the file, where a raw read of the database would still find it (RD-190-04).
            .pragma("secure_delete", "ON")
            .busy_timeout(Duration::from_secs(5))
            .log_statements(LevelFilter::Trace);

        // Free pages can be handed back to the file system a few at a time
        // (`PRAGMA incremental_vacuum`, RD-1240-35). A new file has it from its first table; an
        // older one, created without, takes it on with its next `VACUUM`. The writer alone sets
        // it: on a file that already has tables the pragma writes the header, and a reader
        // connection the pool opens later doing so commits under the writer's open snapshot,
        // which then fails with `SQLITE_BUSY_SNAPSHOT` (517).
        let writer_options = options.clone().auto_vacuum(SqliteAutoVacuum::Incremental);
        let writer_connection = SqliteConnection::connect_with(&writer_options)
            .await
            .context("open SQLite writer connection")?;
        let writer_connection = pre_migration::migrate(writer_connection, path, migrator).await?;

        let readers = SqlitePoolOptions::new()
            .max_connections(4)
            .min_connections(1)
            .connect_with(options)
            .await
            .context("open SQLite reader pool")?;

        let events = EventBus::new();
        let (command_tx, command_rx) = mpsc::channel(256);
        tokio::spawn(Writer::new(writer_connection, command_rx, events.clone()).run());

        Ok(Self {
            readers,
            writer: command_tx,
            events,
            vault: Arc::new(OnceLock::new()),
            path: Arc::new(path.to_path_buf()),
        })
    }

    /// The database file this facade opened.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Hands the database the vault it puts secret link fragments in (RD-110-38).
    ///
    /// Without it intake behaves exactly as it did before: a fragment is dropped, whatever
    /// the provider declared. That is the right degradation -- a key nothing can read back is
    /// worse than no key -- and it is what `rdownloader doctor` and the plugin CLI run with.
    pub fn install_secret_vault(&self, vault: rd_secrets::SecretStore) {
        let _ = self.vault.set(vault);
    }

    /// Installs a vault whose master key lives in a file beside it ([`rd_secrets::SecretStore::open`]),
    /// for a test or a tool that writes archive passwords without the service's vault. Never
    /// for `serve`: the service's master key is in the OS keyring, and a second key would not
    /// open what the first one wrote.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn install_file_vault(&self, root: std::path::PathBuf) -> Result<()> {
        self.install_secret_vault(rd_secrets::SecretStore::open(root).await?);
        Ok(())
    }

    /// The installed vault, if there is one.
    #[must_use]
    pub fn secret_vault(&self) -> Option<&rd_secrets::SecretStore> {
        self.vault.get()
    }

    /// Removes vaulted material whose owning row is gone. Never fails an operation: the row
    /// is already deleted, and a secret that could not be removed is a warning, not a reason
    /// to report the deletion as failed.
    pub(crate) async fn forget_secrets(&self, references: Vec<String>) {
        let Some(vault) = self.vault.get() else {
            return;
        };
        for reference in references {
            if let Err(error) = vault.remove(&reference).await {
                tracing::warn!(%error, "a vaulted link fragment could not be removed");
            }
        }
    }

    /// Atomically replaces every server-side configuration table while preserving supplied ids.
    ///
    /// An account or stream channel whose id the bundle names again keeps its sign-in, remote
    /// jobs and recording schedules (DB-01), a subscription its archive, runs and priming
    /// (RA-DB-04); the sign-ins of accounts it no longer names leave the vault with them.
    pub async fn replace_config(&self, replacement: ConfigReplacement) -> Result<()> {
        let released = writer::request(&self.writer, |reply| MaintenanceCommand::ReplaceConfig {
            replacement,
            reply,
        })
        .await?;
        self.forget_secrets(released).await;
        // The archives of the subscriptions it no longer names went, and their archive
        // passwords with them (RD-190-04).
        self.sweep_archive_passwords().await;
        Ok(())
    }

    /// Resets interrupted active states to queued during startup recovery.
    pub async fn recover_interrupted(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            MaintenanceCommand::RecoverInterrupted { reply }
        })
        .await
    }

    /// Checkpoints the WAL after all writer commands already sent have completed.
    pub async fn checkpoint_wal(&self) -> Result<()> {
        writer::request(&self.writer, |reply| MaintenanceCommand::CheckpointWal {
            reply,
        })
        .await
    }

    /// Closes the read pool and the writer's connection, so the database files are free while
    /// clones of this handle still exist — on Windows a file another handle holds open cannot
    /// be moved. Every later call on any clone fails.
    ///
    /// # Errors
    ///
    /// When the writer is already gone or its connection does not close cleanly.
    pub async fn close(&self) -> Result<()> {
        self.readers.close().await;
        writer::request(&self.writer, |reply| WriterCommand::Close { reply }).await
    }
}
