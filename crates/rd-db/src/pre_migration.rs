//! The copy of the database a start takes before it migrates it (RD-170-07, the core of
//! RD-180-03).
//!
//! sqlx applies each pending migration in a transaction of its own, so a failing migration
//! leaves nothing of itself — but every migration before it in the same start has committed.
//! Such a file has migrations the previous build does not know, and the previous build refuses
//! to open it. So before the first pending migration runs on an existing database, the start
//! writes a consistent copy with `VACUUM INTO` (the snapshot command a full backup uses, here on
//! the writer connection before the writer starts) to
//! `<data>/pre-migration/rdownloader-<from>-to-<to>-<timestamp>.sqlite3`. A migration that fails
//! puts that copy back in place of the database and stops the start with
//! [`MigrationFailure`], which names the copy and the way back: the previous version starts on
//! the file as it was.
//!
//! The copy is not encrypted: it is the same file the service keeps unencrypted beside it.
//! Credentials are references into the vault (`<data>/secrets`), sign-in sessions and capture
//! tokens are stored as SHA-256 hashes; what the database itself holds in plain — archive
//! passwords of packages among it — the copy holds too, no more.
//!
//! A fresh database gets no copy, and neither does one from a newer build, which sqlx refuses
//! before it applies anything. The newest [`KEPT`] copies stay; older ones are removed whenever
//! a start leaves the database whole — upgraded, or put back from its copy — so a service that
//! restarts into the same failure does not fill the disk, and never after a put-back that
//! failed, which would rotate away the copy the database has to be restored from by hand.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sqlx::{Connection, SqliteConnection, migrate::Migrator};

use crate::restore_copy::{CopySchema, schema_on};

/// The folder below the data directory the copies are written to.
pub const DIRECTORY: &str = "pre-migration";

/// How many copies stay.
pub const KEPT: usize = 3;

/// A migration failed; the database was put back from the copy, or could not be.
pub const MIGRATION_FAILED: &str = "db.migration_failed";

/// The copy could not be written, so nothing was migrated.
pub const SNAPSHOT_FAILED: &str = "db.pre_migration_snapshot_failed";

const PREFIX: &str = "rdownloader-";
const EXTENSION: &str = ".sqlite3";

/// Why a start did not migrate its database, with a stable code and the way back.
#[derive(Debug)]
pub struct MigrationFailure {
    /// [`MIGRATION_FAILED`] or [`SNAPSHOT_FAILED`].
    pub code: &'static str,
    /// The database file.
    pub database: PathBuf,
    /// The copy taken before the first migration; `None` when it could not be written.
    pub snapshot: Option<PathBuf>,
    /// Whether the database is the copy again. Only meaningful for [`MIGRATION_FAILED`].
    pub restored: bool,
    /// What failed, the migration's own error first.
    pub detail: String,
}

impl std::fmt::Display for MigrationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let database = self.database.display();
        match (&self.snapshot, self.restored) {
            (None, _) => write!(
                formatter,
                "{}: the copy of {database} to take before the upgrade could not be written, so \
                 nothing was migrated and the database is as it was: {}",
                self.code, self.detail
            ),
            (Some(snapshot), true) => write!(
                formatter,
                "{}: the database upgrade failed and {database} was put back as it was before \
                 it; the previous rDownloader version starts on it again. The copy it was put \
                 back from is {}. Cause: {}",
                self.code,
                snapshot.display(),
                self.detail
            ),
            (Some(snapshot), false) => write!(
                formatter,
                "{}: the database upgrade failed and {database} could not be put back \
                 automatically. Stop rDownloader, delete {database}-wal and {database}-shm, \
                 replace {database} with {}, then start the previous rDownloader version. \
                 Cause: {}",
                self.code,
                snapshot.display(),
                self.detail
            ),
        }
    }
}

impl std::error::Error for MigrationFailure {}

/// Applies `migrator`'s pending migrations to the database at `path`, open on `connection`,
/// with the copy before them; hands the connection back when every migration applied.
///
/// # Errors
///
/// [`MigrationFailure`] when the copy could not be written or a migration failed; any other
/// error when the applied migrations cannot be read.
pub(crate) async fn migrate(
    mut connection: SqliteConnection,
    path: &Path,
    migrator: &Migrator,
) -> Result<SqliteConnection> {
    let schema = schema_on(&mut connection, migrator)
        .await
        .context("read the applied migrations")?;
    let Some(from) = schema.applied.filter(|_| needs_copy(&schema)) else {
        migrator
            .run(&mut connection)
            .await
            .context("apply SQLite migrations")?;
        return Ok(connection);
    };
    let directory = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(DIRECTORY);
    let snapshot = directory.join(snapshot_name(from, schema.known, chrono::Utc::now()));
    if let Err(error) = write_snapshot(&mut connection, &directory, &snapshot).await {
        return Err(MigrationFailure {
            code: SNAPSHOT_FAILED,
            database: path.to_path_buf(),
            snapshot: None,
            restored: false,
            detail: format!("{error:#}"),
        }
        .into());
    }
    tracing::info!(
        snapshot = %snapshot.display(),
        from,
        to = schema.known,
        "database copied before its upgrade"
    );
    match migrator.run(&mut connection).await {
        Ok(()) => {
            rotate(&directory, KEPT).await;
            Ok(connection)
        }
        Err(error) => {
            let mut detail = format!("{error}");
            if let Err(close) = connection.close().await {
                tracing::warn!(error = %close, "the database did not close cleanly after a failed migration");
            }
            let restored = match put_back(path, &snapshot).await {
                Ok(()) => {
                    rotate(&directory, KEPT).await;
                    true
                }
                Err(restore) => {
                    detail = format!("{detail}; putting the copy back failed: {restore:#}");
                    false
                }
            };
            tracing::error!(snapshot = %snapshot.display(), restored, %detail, "a database migration failed");
            Err(MigrationFailure {
                code: MIGRATION_FAILED,
                database: path.to_path_buf(),
                snapshot: Some(snapshot),
                restored,
                detail,
            }
            .into())
        }
    }
}

/// An existing database with something to apply, written by this build or an older one.
fn needs_copy(schema: &CopySchema) -> bool {
    schema.applied.is_some() && schema.pending > 0 && !schema.is_newer()
}

/// `rdownloader-<from>-to-<to>-<timestamp>.sqlite3`; the timestamp is the last segment, fixed
/// width, so the names of one folder sort by it.
fn snapshot_name(from: i64, to: i64, at: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "{PREFIX}{from:04}-to-{to:04}-{}{EXTENSION}",
        at.format("%Y%m%dT%H%M%S%3fZ")
    )
}

/// The timestamp of a name [`snapshot_name`] wrote; `None` for every other file.
fn snapshot_stamp(name: &str) -> Option<&str> {
    let stem = name.strip_prefix(PREFIX)?.strip_suffix(EXTENSION)?;
    let (versions, stamp) = stem.rsplit_once('-')?;
    (versions.contains("-to-") && stamp.len() == 19 && stamp.ends_with('Z')).then_some(stamp)
}

async fn write_snapshot(
    connection: &mut SqliteConnection,
    directory: &Path,
    snapshot: &Path,
) -> Result<()> {
    tokio::fs::create_dir_all(directory)
        .await
        .with_context(|| format!("create {}", directory.display()))?;
    let target = snapshot
        .to_str()
        .with_context(|| format!("snapshot path {} is not UTF-8", snapshot.display()))?;
    sqlx::query("VACUUM INTO ?")
        .bind(target)
        .execute(&mut *connection)
        .await
        .with_context(|| format!("write database copy {}", snapshot.display()))?;
    Ok(())
}

/// Replaces the database with the copy. The copy is first duplicated beside the database and
/// synced, so the one step that changes the database is a rename; the journal files go before
/// it, since a journal left beside the put-back file would be replayed into it.
async fn put_back(path: &Path, snapshot: &Path) -> Result<()> {
    let mut staged = path.as_os_str().to_owned();
    staged.push(".restoring");
    let staged = PathBuf::from(staged);
    tokio::fs::copy(snapshot, &staged)
        .await
        .with_context(|| format!("copy {} beside the database", snapshot.display()))?;
    // Opened for writing: Windows flushes a file only through a handle with write access.
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(&staged)
        .await?
        .sync_all()
        .await
        .with_context(|| format!("sync {}", staged.display()))?;
    for suffix in ["-wal", "-shm"] {
        let mut journal = path.as_os_str().to_owned();
        journal.push(suffix);
        match tokio::fs::remove_file(PathBuf::from(journal)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove the database journal"),
        }
    }
    tokio::fs::rename(&staged, path)
        .await
        .with_context(|| format!("put {} back in place", snapshot.display()))?;
    Ok(())
}

/// Removes every copy but the newest `keep`. Files this module did not name are left alone,
/// and a copy that cannot be removed is a warning for the next pass.
pub(crate) async fn rotate(directory: &Path, keep: usize) {
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return;
    };
    let mut copies = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(stamp) = snapshot_stamp(&name) {
            copies.push((stamp.to_owned(), entry.path()));
        }
    }
    copies.sort();
    let surplus = copies.len().saturating_sub(keep);
    for (_, path) in copies.into_iter().take(surplus) {
        if let Err(error) = tokio::fs::remove_file(&path).await {
            tracing::warn!(%error, copy = %path.display(), "an old pre-migration copy could not be removed");
        }
    }
}
