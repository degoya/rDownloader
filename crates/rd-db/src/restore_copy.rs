//! The database copy a full backup carries, opened for a restore (RD-160-03).
//!
//! A restore never opens the copy as the live database. It reads which migrations wrote the
//! copy, refuses one from a newer build, migrates an older one in place — the copy is the
//! restore's own file in its own folder — and rewrites the cells a restore has to change: the
//! paths below a storage root that moved, and the references into a secret store the copy did
//! not come with. Every table and column name that reaches a statement comes from the fixed
//! lists below, never from a request.
//!
//! The copy is kept in rollback-journal mode, so it stays one file the cutover can move.

use std::path::Path;

use anyhow::{Context, Result};
use sqlx::{
    Connection, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};

/// One column of the copy a restore reads or rewrites, with the column that names its rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyColumn {
    pub table: &'static str,
    pub column: &'static str,
    /// `rowid` for the path columns; `id` where a row has to be matched to a bundle entry.
    pub key: &'static str,
}

const fn column(table: &'static str, name: &'static str, key: &'static str) -> CopyColumn {
    CopyColumn {
        table,
        column: name,
        key,
    }
}

/// A storage root's own path: what a remap names.
pub const STORAGE_ROOT_PATH: CopyColumn = column("storage_roots", "path", "id");

/// Every other column that holds an absolute path on the machine the backup was made on.
pub const PATH_COLUMNS: &[CopyColumn] = &[
    column("packages", "destination", "rowid"),
    column("packages", "previous_destination", "rowid"),
    column("hotfolders", "path", "rowid"),
    column("hotfolders", "processed_path", "rowid"),
    column("hotfolders", "failed_path", "rowid"),
    column("content_index", "path", "rowid"),
    column("postprocess_steps", "source_path", "rowid"),
    column("postprocess_steps", "output_path", "rowid"),
    column("nzb_imports", "source_path", "rowid"),
    column("nzb_files", "output_path", "rowid"),
    column("storage_operations", "source_path", "rowid"),
    column("storage_operations", "target_path", "rowid"),
    column("object_uploads", "local_path", "rowid"),
];

/// The queue's sources; a stored `.torrent` is a `file://` URL into the data directory.
pub const DOWNLOAD_SOURCE: CopyColumn = column("downloads", "source_url", "rowid");

/// References whose values the settings bundle inside the archive carries, by row id.
pub const BUNDLED_SECRET_COLUMNS: &[CopyColumn] = &[
    column("accounts", "secret_ref", "id"),
    column("accounts", "cookie_ref", "id"),
    column("proxy_profiles", "secret_ref", "id"),
    column("usenet_servers", "password_ref", "id"),
    column("subscriptions", "secret_ref", "id"),
    column("auth_profiles", "secret_ref", "id"),
    column("auth_profiles", "certificate_ref", "id"),
    column("indexers", "secret_ref", "id"),
    // RD-190-04: archive passwords, sealed in the full backup's bundle under its own key (the
    // settings export never carries them). They stay the last four: `restore_checks` pairs
    // them with the bundle by their place at the end.
    column("packages", "password_ref", "id"),
    column("collector_packages", "password_ref", "id"),
    column("nzb_imports", "password_ref", "id"),
    column("subscription_items", "password_ref", "id"),
];

/// References the bundle deliberately does not carry (the second factor, sign-in sessions,
/// vaulted link fragments) or that were added after it: on another machine they point nowhere.
pub const UNBUNDLED_SECRET_COLUMNS: &[CopyColumn] = &[
    column("notification_targets", "secret_ref", "rowid"),
    column("remote_credentials", "secret_ref", "rowid"),
    column("remote_credentials", "key_ref", "rowid"),
    column("remote_credentials", "passphrase_ref", "rowid"),
    column("object_storage_profiles", "secret_ref", "rowid"),
    column("object_storage_profiles", "session_token_ref", "rowid"),
    column("mfa_credentials", "material_ref", "rowid"),
    column("auth_flows", "access_ref", "rowid"),
    column("auth_flows", "refresh_ref", "rowid"),
    column("auth_flows", "key_ref", "rowid"),
    column("auth_flow_parts", "secret_ref", "rowid"),
    column("link_candidates", "secret_fragment_ref", "rowid"),
    column("link_candidates", "replay_body_ref", "rowid"),
    column("downloads", "secret_fragment_ref", "rowid"),
    column("downloads", "transform_key_ref", "rowid"),
    column("download_request_templates", "body_ref", "rowid"),
    // RD-1240-13: the VAPID key. On another machine the service makes a new one, and the
    // browsers turn push on again.
    column("web_push_keys", "private_key_ref", "rowid"),
];

/// The backup key's reference, which a restore can put back: it knows the key.
pub const BACKUP_KEY_REF: CopyColumn = column("backup_config", "key_ref", "id");

/// Which migrations wrote the copy, against the ones this build knows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopySchema {
    /// The highest migration the copy has applied; `None` for a file without the table.
    pub applied: Option<i64>,
    /// The highest migration this build knows.
    pub known: i64,
    /// Applied migrations this build does not know: the copy is from a newer build.
    pub unknown: Vec<i64>,
    /// Known migrations the copy has not applied yet.
    pub pending: usize,
}

impl CopySchema {
    /// Whether a newer build wrote the copy; such a copy is never migrated or restored.
    #[must_use]
    pub fn is_newer(&self) -> bool {
        !self.unknown.is_empty()
    }
}

/// How much the copy holds, for the preview.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CopyCounts {
    pub packages: u64,
    pub downloads: u64,
    pub unfinished: u64,
    pub torrents: u64,
    pub storage_roots: u64,
    pub categories: u64,
    pub accounts: u64,
    pub hotfolders: u64,
}

/// One non-empty cell of a [`CopyColumn`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyCell {
    pub key: String,
    pub value: String,
}

/// One cell to write: `None` clears it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyUpdate {
    pub column: CopyColumn,
    pub key: String,
    pub value: Option<String>,
}

/// Read-only, like the snapshot reader: no journal mode is asked for, which would be a write.
async fn open_read_only(copy: &Path) -> Result<SqliteConnection> {
    let options = SqliteConnectOptions::new()
        .filename(copy)
        .read_only(true)
        .create_if_missing(false);
    SqliteConnection::connect_with(&options)
        .await
        .with_context(|| format!("open database copy {}", copy.display()))
}

fn writable_options(copy: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(copy)
        .create_if_missing(false)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Delete)
}

async fn open_writable(copy: &Path) -> Result<SqliteConnection> {
    SqliteConnection::connect_with(&writable_options(copy))
        .await
        .with_context(|| format!("open database copy {}", copy.display()))
}

/// Which of `migrator`'s migrations the database on `connection` has applied; the start reads
/// the same before it migrates the live database (`crate::pre_migration`).
pub(crate) async fn schema_on(
    connection: &mut SqliteConnection,
    migrator: &sqlx::migrate::Migrator,
) -> Result<CopySchema> {
    let known_versions: Vec<i64> = migrator.iter().map(|migration| migration.version).collect();
    let has_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&mut *connection)
    .await
    .context("read the copy's tables")?;
    let applied: Vec<i64> = if has_table == 0 {
        Vec::new()
    } else {
        sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations WHERE success = 1 ORDER BY version",
        )
        .fetch_all(&mut *connection)
        .await
        .context("read the copy's migrations")?
    };
    Ok(CopySchema {
        applied: applied.last().copied(),
        known: known_versions.last().copied().unwrap_or_default(),
        unknown: applied
            .iter()
            .copied()
            .filter(|version| !known_versions.contains(version))
            .collect(),
        pending: known_versions
            .iter()
            .filter(|version| !applied.contains(version))
            .count(),
    })
}

/// Which migrations wrote the copy. Read-only.
///
/// # Errors
///
/// When the file is no SQLite database.
pub async fn copy_schema(copy: &Path) -> Result<CopySchema> {
    let mut connection = open_read_only(copy).await?;
    let schema = schema_on(&mut connection, &crate::MIGRATOR).await;
    connection.close().await.ok();
    schema
}

/// Applies this build's pending migrations to the copy and returns the schema it had before.
/// A copy from a newer build is refused untouched.
///
/// # Errors
///
/// When the copy is from a newer build, or a migration fails on it.
pub async fn migrate_copy(copy: &Path) -> Result<CopySchema> {
    let mut connection = open_writable(copy).await?;
    let schema = schema_on(&mut connection, &crate::MIGRATOR).await?;
    connection.close().await.ok();
    if schema.is_newer() {
        anyhow::bail!(
            "the database copy was written by a newer rDownloader (migrations {:?})",
            schema.unknown
        );
    }
    // Through a one-connection pool: `Migrator::run` on a `&mut SqliteConnection` gives a
    // future that is not `Send` for every lifetime, which an axum handler needs.
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(writable_options(copy))
        .await
        .with_context(|| format!("open database copy {}", copy.display()))?;
    let migrated = crate::MIGRATOR
        .run(&pool)
        .await
        .context("apply the migrations to the database copy");
    pool.close().await;
    migrated?;
    Ok(schema)
}

async fn count(connection: &mut SqliteConnection, statement: &'static str) -> Result<u64> {
    let value: i64 = sqlx::query_scalar(statement)
        .fetch_one(&mut *connection)
        .await
        .with_context(|| format!("count: {statement}"))?;
    Ok(u64::try_from(value).unwrap_or_default())
}

/// How much the copy holds. Read-only; the tables named here exist since `0001`, `0004`.
///
/// # Errors
///
/// When a table cannot be read.
pub async fn copy_counts(copy: &Path) -> Result<CopyCounts> {
    let mut connection = open_read_only(copy).await?;
    let counts = CopyCounts {
        packages: count(&mut connection, "SELECT COUNT(*) FROM packages").await?,
        downloads: count(&mut connection, "SELECT COUNT(*) FROM downloads").await?,
        unfinished: count(
            &mut connection,
            "SELECT COUNT(*) FROM downloads WHERE state NOT IN ('completed', 'cancelled')",
        )
        .await?,
        torrents: count(
            &mut connection,
            "SELECT COUNT(*) FROM downloads WHERE kind = 'torrent'",
        )
        .await?,
        storage_roots: count(&mut connection, "SELECT COUNT(*) FROM storage_roots").await?,
        categories: count(&mut connection, "SELECT COUNT(*) FROM categories").await?,
        accounts: count(&mut connection, "SELECT COUNT(*) FROM accounts").await?,
        hotfolders: count(&mut connection, "SELECT COUNT(*) FROM hotfolders").await?,
    };
    connection.close().await.ok();
    Ok(counts)
}

/// Every non-empty cell of `columns`, each list in the order given. Read-only.
///
/// # Errors
///
/// When a column cannot be read — on a copy that was not migrated, a table may be missing.
pub async fn read_cells(copy: &Path, columns: &[CopyColumn]) -> Result<Vec<Vec<CopyCell>>> {
    let mut connection = open_read_only(copy).await?;
    let mut all = Vec::with_capacity(columns.len());
    for target in columns {
        let rows: Vec<(String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT CAST({key} AS TEXT), {column} FROM {table} \
             WHERE {column} IS NOT NULL AND {column} != '' ORDER BY rowid",
            key = target.key,
            column = target.column,
            table = target.table,
        )))
        .fetch_all(&mut connection)
        .await
        .with_context(|| format!("read {}.{}", target.table, target.column))?;
        all.push(
            rows.into_iter()
                .map(|(key, value)| CopyCell { key, value })
                .collect(),
        );
    }
    connection.close().await.ok();
    Ok(all)
}

/// Writes every update in one transaction: all of them land, or none.
///
/// # Errors
///
/// When a statement fails, a constraint refuses a value, or a key names no row.
pub async fn apply_updates(copy: &Path, updates: &[CopyUpdate]) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut connection = open_writable(copy).await?;
    let mut transaction = connection.begin().await?;
    for update in updates {
        let target = update.column;
        let result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET {column} = ? WHERE {key} = ?",
            table = target.table,
            column = target.column,
            key = target.key,
        )))
        .bind(update.value.as_deref())
        .bind(&update.key)
        .execute(&mut *transaction)
        .await
        .with_context(|| format!("write {}.{}", target.table, target.column))?;
        anyhow::ensure!(
            result.rows_affected() == 1,
            "{}.{} has no row {}",
            target.table,
            target.column,
            update.key
        );
    }
    transaction.commit().await?;
    connection.close().await.ok();
    Ok(())
}

/// Clears the backup key of a copy whose key this restore cannot put back, and switches its
/// schedule off: a schedule without a key only fails.
///
/// # Errors
///
/// When the row cannot be written.
pub async fn clear_backup_key(copy: &Path) -> Result<()> {
    let mut connection = open_writable(copy).await?;
    sqlx::query(
        "UPDATE backup_config SET enabled = 0, key_ref = NULL, key_salt = NULL, \
         key_fingerprint = NULL, key_set_at = NULL, next_run_at = NULL",
    )
    .execute(&mut connection)
    .await
    .context("clear the copy's backup key")?;
    connection.close().await.ok();
    Ok(())
}

/// The backup key the copy names, as `(fingerprint, reference)`, if it has one.
///
/// # Errors
///
/// When the row cannot be read.
pub async fn backup_key_of(copy: &Path) -> Result<Option<(String, String)>> {
    let mut connection = open_read_only(copy).await?;
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT key_fingerprint, key_ref FROM backup_config WHERE id = 1")
            .fetch_optional(&mut connection)
            .await
            .context("read the copy's backup key")?;
    connection.close().await.ok();
    Ok(row.and_then(|(fingerprint, reference)| fingerprint.zip(reference)))
}

/// A row whose foreign key names no row: `(table, parent table, rows)`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DanglingReference {
    pub table: String,
    pub parent: String,
    pub rows: u64,
}

/// Every foreign key of the copy that names no row, by table and parent: categories without
/// their storage root, hot folders without their category, and every other reference SQLite
/// knows of. Read-only.
///
/// # Errors
///
/// When the check cannot run.
pub async fn dangling_references(copy: &Path) -> Result<Vec<DanglingReference>> {
    let mut connection = open_read_only(copy).await?;
    let rows: Vec<(String, Option<i64>, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&mut connection)
        .await
        .context("check the copy's references")?;
    connection.close().await.ok();
    let mut found: Vec<DanglingReference> = Vec::new();
    for (table, _, parent, _) in rows {
        match found
            .iter_mut()
            .find(|entry| entry.table == table && entry.parent == parent)
        {
            Some(entry) => entry.rows += 1,
            None => found.push(DanglingReference {
                table,
                parent,
                rows: 1,
            }),
        }
    }
    Ok(found)
}

/// The folders of the packages that still have unfinished downloads: where their part files
/// are expected. Read-only.
///
/// # Errors
///
/// When the tables cannot be read.
pub async fn unfinished_destinations(copy: &Path) -> Result<Vec<String>> {
    let mut connection = open_read_only(copy).await?;
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT p.destination FROM packages p JOIN downloads d ON d.package_id = p.id \
         WHERE d.state NOT IN ('completed', 'cancelled') ORDER BY p.destination",
    )
    .fetch_all(&mut connection)
    .await
    .context("read the unfinished downloads' folders")?;
    connection.close().await.ok();
    Ok(rows)
}

#[path = "restore_copy_schema.rs"]
mod schema;

pub use schema::foreign_schema_objects;

#[cfg(test)]
#[path = "restore_copy_tests.rs"]
mod tests;
