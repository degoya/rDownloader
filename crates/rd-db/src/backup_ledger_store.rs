//! The full backup's destinations, the ledger of archives written to them, how each
//! destination of a run fared, and the verifications (RD-160-02).
//!
//! The ledger is what makes retention safe: it lists every archive this installation placed,
//! with the size and SHA-256 it had, and retention deletes nothing that is not in it.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{BackupOrigin, BackupRunState, BackupVerifyState};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::full_backup_store::{BackupDestinationRecord, NewBackupDestination};

/// How many verifications the history keeps.
pub const BACKUP_VERIFICATIONS_KEPT: i64 = 100;

/// One archive at one destination, as it was written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupArchive {
    pub id: String,
    pub destination_id: String,
    pub run_id: String,
    pub archive_name: String,
    pub location: String,
    pub size_bytes: u64,
    pub sha256: String,
    /// When the run that wrote it started: the archive's point in time.
    pub created_at: DateTime<Utc>,
    pub stored_at: DateTime<Utc>,
    pub verified_at: Option<DateTime<Utc>>,
    /// `passed` or `failed`, from the last verification.
    pub verify_state: Option<BackupVerifyState>,
    pub verify_code: Option<String>,
}

/// An archive just placed at a destination.
#[derive(Clone, Debug)]
pub struct NewBackupArchive {
    pub destination_id: String,
    pub run_id: String,
    pub archive_name: String,
    pub location: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
}

/// How one destination of a run fared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupRunDestination {
    pub destination_id: String,
    pub kind: String,
    /// The destination as it was named when the run started.
    pub destination: String,
    pub state: BackupRunState,
    pub attempts: u32,
    pub location: Option<String>,
    /// Older archives retention removed there after this run.
    pub pruned: u32,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// The end of one destination's delivery.
#[derive(Clone, Debug)]
pub struct BackupRunDestinationEnd {
    pub run_id: String,
    pub destination_id: String,
    /// `Succeeded` or `Failed`.
    pub state: BackupRunState,
    pub attempts: u32,
    pub location: Option<String>,
    pub pruned: u32,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
}

/// One verification of one archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupVerification {
    pub id: String,
    pub origin: BackupOrigin,
    pub state: BackupVerifyState,
    pub archive_id: Option<String>,
    pub destination_id: Option<String>,
    pub destination: String,
    pub archive_name: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub content_checked: Option<bool>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
}

/// How a verification ended.
#[derive(Clone, Debug)]
pub enum BackupVerificationOutcome {
    Passed { content_checked: bool },
    Failed { code: String, detail: String },
}

#[derive(FromRow)]
struct DestinationRow {
    id: String,
    kind: String,
    name: String,
    config_json: String,
    enabled: bool,
    keep_last: Option<i64>,
    keep_days: Option<i64>,
    created_at: DateTime<Utc>,
}

impl TryFrom<DestinationRow> for BackupDestinationRecord {
    type Error = anyhow::Error;

    fn try_from(row: DestinationRow) -> Result<Self> {
        Ok(Self {
            config: serde_json::from_str(&row.config_json)
                .context("parse stored backup destination")?,
            id: row.id,
            kind: row.kind,
            name: row.name,
            enabled: row.enabled,
            keep_last: row.keep_last.and_then(|value| u32::try_from(value).ok()),
            keep_days: row.keep_days.and_then(|value| u32::try_from(value).ok()),
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
struct ArchiveRow {
    id: String,
    destination_id: String,
    run_id: String,
    archive_name: String,
    location: String,
    size_bytes: i64,
    sha256: String,
    created_at: DateTime<Utc>,
    stored_at: DateTime<Utc>,
    verified_at: Option<DateTime<Utc>>,
    verify_state: Option<String>,
    verify_code: Option<String>,
}

impl From<ArchiveRow> for BackupArchive {
    fn from(row: ArchiveRow) -> Self {
        Self {
            id: row.id,
            destination_id: row.destination_id,
            run_id: row.run_id,
            archive_name: row.archive_name,
            location: row.location,
            size_bytes: u64::try_from(row.size_bytes).unwrap_or_default(),
            sha256: row.sha256,
            created_at: row.created_at,
            stored_at: row.stored_at,
            verified_at: row.verified_at,
            verify_state: row
                .verify_state
                .as_deref()
                .and_then(BackupVerifyState::parse),
            verify_code: row.verify_code,
        }
    }
}

#[derive(FromRow)]
struct RunDestinationRow {
    destination_id: String,
    kind: String,
    destination: String,
    state: String,
    attempts: i64,
    location: Option<String>,
    pruned: i64,
    error_code: Option<String>,
    error_detail: Option<String>,
    finished_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct VerificationRow {
    id: String,
    origin: String,
    state: String,
    archive_id: Option<String>,
    destination_id: Option<String>,
    destination: String,
    archive_name: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    content_checked: Option<bool>,
    error_code: Option<String>,
    error_detail: Option<String>,
}

const DESTINATION_COLUMNS: &str =
    "id, kind, name, config_json, enabled, keep_last, keep_days, created_at";
const ARCHIVE_COLUMNS: &str = "id, destination_id, run_id, archive_name, location, size_bytes, \
     sha256, created_at, stored_at, verified_at, verify_state, verify_code";

pub(crate) async fn destinations(pool: &SqlitePool) -> Result<Vec<BackupDestinationRecord>> {
    sqlx::query_as::<_, DestinationRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {DESTINATION_COLUMNS} FROM backup_destinations ORDER BY created_at, id"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn destination(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<BackupDestinationRecord>> {
    sqlx::query_as::<_, DestinationRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {DESTINATION_COLUMNS} FROM backup_destinations WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn create_destination(
    connection: &mut SqliteConnection,
    destination: NewBackupDestination,
) -> Result<String> {
    let id = uuid::Uuid::now_v7().to_string();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO backup_destinations (id, kind, name, config_json, enabled, keep_last, \
         keep_days, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&destination.kind)
    .bind(&destination.name)
    .bind(serde_json::to_string(&destination.config)?)
    .bind(destination.enabled)
    .bind(destination.keep_last.map(i64::from))
    .bind(destination.keep_days.map(i64::from))
    .bind(now)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(id)
}

/// Replaces a destination; `false` when there is none of that id.
pub(crate) async fn update_destination(
    connection: &mut SqliteConnection,
    id: &str,
    destination: NewBackupDestination,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE backup_destinations SET kind = ?, name = ?, config_json = ?, enabled = ?, \
         keep_last = ?, keep_days = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&destination.kind)
    .bind(&destination.name)
    .bind(serde_json::to_string(&destination.config)?)
    .bind(destination.enabled)
    .bind(destination.keep_last.map(i64::from))
    .bind(destination.keep_days.map(i64::from))
    .bind(Utc::now())
    .bind(id)
    .execute(connection)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Removes a destination and forgets its archives; the archives stay where they are.
pub(crate) async fn delete_destination(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<bool> {
    let result = sqlx::query("DELETE FROM backup_destinations WHERE id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Records an archive placed at a destination. The same name at the same destination is one
/// archive: a second record replaces the first.
pub(crate) async fn record_archive(
    connection: &mut SqliteConnection,
    archive: NewBackupArchive,
) -> Result<String> {
    let id = uuid::Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO backup_archives (id, destination_id, run_id, archive_name, location, \
         size_bytes, sha256, created_at, stored_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT (destination_id, archive_name) DO UPDATE SET id = excluded.id, \
         run_id = excluded.run_id, location = excluded.location, \
         size_bytes = excluded.size_bytes, sha256 = excluded.sha256, \
         created_at = excluded.created_at, stored_at = excluded.stored_at, \
         verified_at = NULL, verify_state = NULL, verify_code = NULL",
    )
    .bind(&id)
    .bind(&archive.destination_id)
    .bind(&archive.run_id)
    .bind(&archive.archive_name)
    .bind(&archive.location)
    .bind(i64::try_from(archive.size_bytes).unwrap_or(i64::MAX))
    .bind(&archive.sha256)
    .bind(archive.created_at)
    .bind(Utc::now())
    .execute(connection)
    .await?;
    Ok(id)
}

/// The ledger, newest first; of one destination or of all.
pub(crate) async fn archives(
    pool: &SqlitePool,
    destination_id: Option<&str>,
) -> Result<Vec<BackupArchive>> {
    let rows = match destination_id {
        Some(id) => {
            sqlx::query_as::<_, ArchiveRow>(sqlx::AssertSqlSafe(format!(
                "SELECT {ARCHIVE_COLUMNS} FROM backup_archives WHERE destination_id = ? \
                 ORDER BY created_at DESC, archive_name DESC"
            )))
            .bind(id)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, ArchiveRow>(sqlx::AssertSqlSafe(format!(
                "SELECT {ARCHIVE_COLUMNS} FROM backup_archives \
                 ORDER BY created_at DESC, archive_name DESC"
            )))
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows.into_iter().map(Into::into).collect())
}

pub(crate) async fn archive(pool: &SqlitePool, id: &str) -> Result<Option<BackupArchive>> {
    Ok(sqlx::query_as::<_, ArchiveRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {ARCHIVE_COLUMNS} FROM backup_archives WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .map(Into::into))
}

/// Forgets archives retention removed; returns how many rows went.
pub(crate) async fn forget_archives(
    connection: &mut SqliteConnection,
    ids: Vec<String>,
) -> Result<u64> {
    let mut tx = connection.begin().await?;
    let mut removed = 0;
    for id in ids {
        removed += sqlx::query("DELETE FROM backup_archives WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    }
    tx.commit().await?;
    Ok(removed)
}

/// Records the start of a run's deliveries, one row per destination.
pub(crate) async fn begin_run_destinations(
    connection: &mut SqliteConnection,
    run_id: &str,
    destinations: Vec<(String, String, String)>,
) -> Result<()> {
    let mut tx = connection.begin().await?;
    for (destination_id, kind, label) in destinations {
        sqlx::query(
            "INSERT INTO backup_run_destinations (run_id, destination_id, kind, destination, \
             state) VALUES (?, ?, ?, ?, 'running')",
        )
        .bind(run_id)
        .bind(destination_id)
        .bind(kind)
        .bind(label)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn finish_run_destination(
    connection: &mut SqliteConnection,
    end: BackupRunDestinationEnd,
) -> Result<()> {
    sqlx::query(
        "UPDATE backup_run_destinations SET state = ?, attempts = ?, location = ?, pruned = ?, \
         error_code = ?, error_detail = ?, finished_at = ? \
         WHERE run_id = ? AND destination_id = ? AND state = 'running'",
    )
    .bind(end.state.as_str())
    .bind(i64::from(end.attempts))
    .bind(end.location)
    .bind(i64::from(end.pruned))
    .bind(end.error_code)
    .bind(end.error_detail)
    .bind(Utc::now())
    .bind(end.run_id)
    .bind(end.destination_id)
    .execute(connection)
    .await?;
    Ok(())
}

pub(crate) async fn run_destinations(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<Vec<BackupRunDestination>> {
    sqlx::query_as::<_, RunDestinationRow>(
        "SELECT destination_id, kind, destination, state, attempts, location, pruned, \
         error_code, error_detail, finished_at FROM backup_run_destinations WHERE run_id = ? \
         ORDER BY rowid",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| -> Result<BackupRunDestination> {
        Ok(BackupRunDestination {
            state: BackupRunState::parse(&row.state)
                .with_context(|| format!("unknown backup state {}", row.state))?,
            destination_id: row.destination_id,
            kind: row.kind,
            destination: row.destination,
            attempts: u32::try_from(row.attempts).unwrap_or_default(),
            location: row.location,
            pruned: u32::try_from(row.pruned).unwrap_or_default(),
            error_code: row.error_code,
            error_detail: row.error_detail,
            finished_at: row.finished_at,
        })
    })
    .collect()
}

/// Records the start of a verification.
pub(crate) async fn begin_verification(
    connection: &mut SqliteConnection,
    verification: BackupVerification,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO backup_verifications (id, origin, state, archive_id, destination_id, \
         destination, archive_name, started_at) VALUES (?, ?, 'running', ?, ?, ?, ?, ?)",
    )
    .bind(&verification.id)
    .bind(verification.origin.as_str())
    .bind(&verification.archive_id)
    .bind(&verification.destination_id)
    .bind(&verification.destination)
    .bind(&verification.archive_name)
    .bind(verification.started_at)
    .execute(connection)
    .await?;
    Ok(())
}

/// Records how a verification ended, on its row and on the archive's, and drops the history
/// beyond [`BACKUP_VERIFICATIONS_KEPT`].
pub(crate) async fn finish_verification(
    connection: &mut SqliteConnection,
    id: &str,
    outcome: BackupVerificationOutcome,
) -> Result<()> {
    let now = Utc::now();
    let (state, content_checked, code, detail) = match outcome {
        BackupVerificationOutcome::Passed { content_checked } => {
            (BackupVerifyState::Passed, Some(content_checked), None, None)
        }
        BackupVerificationOutcome::Failed { code, detail } => {
            (BackupVerifyState::Failed, None, Some(code), Some(detail))
        }
    };
    let mut tx = connection.begin().await?;
    sqlx::query(
        "UPDATE backup_verifications SET state = ?, finished_at = ?, content_checked = ?, \
         error_code = ?, error_detail = ? WHERE id = ? AND state = 'running'",
    )
    .bind(state.as_str())
    .bind(now)
    .bind(content_checked)
    .bind(&code)
    .bind(&detail)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE backup_archives SET verified_at = ?, verify_state = ?, verify_code = ? \
         WHERE id = (SELECT archive_id FROM backup_verifications WHERE id = ?)",
    )
    .bind(now)
    .bind(state.as_str())
    .bind(&code)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM backup_verifications WHERE state != 'running' AND id NOT IN \
         (SELECT id FROM backup_verifications ORDER BY started_at DESC, id LIMIT ?)",
    )
    .bind(BACKUP_VERIFICATIONS_KEPT)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn verifications(
    pool: &SqlitePool,
    limit: u32,
) -> Result<Vec<BackupVerification>> {
    sqlx::query_as::<_, VerificationRow>(
        "SELECT id, origin, state, archive_id, destination_id, destination, archive_name, \
         started_at, finished_at, content_checked, error_code, error_detail \
         FROM backup_verifications ORDER BY started_at DESC, id LIMIT ?",
    )
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| -> Result<BackupVerification> {
        Ok(BackupVerification {
            origin: BackupOrigin::parse(&row.origin)
                .with_context(|| format!("unknown backup origin {}", row.origin))?,
            state: BackupVerifyState::parse(&row.state)
                .with_context(|| format!("unknown verification state {}", row.state))?,
            id: row.id,
            archive_id: row.archive_id,
            destination_id: row.destination_id,
            destination: row.destination,
            archive_name: row.archive_name,
            started_at: row.started_at,
            finished_at: row.finished_at,
            content_checked: row.content_checked,
            error_code: row.error_code,
            error_detail: row.error_detail,
        })
    })
    .collect()
}

/// Sets when the scheduled verification is next due.
pub(crate) async fn arm_verify(
    connection: &mut SqliteConnection,
    next_run_at: Option<DateTime<Utc>>,
) -> Result<()> {
    sqlx::query("UPDATE backup_config SET verify_next_run_at = ? WHERE id = 1")
        .bind(next_run_at)
        .execute(connection)
        .await?;
    Ok(())
}
