//! Persistence of the full backup's configuration and run history (RD-160-01).
//!
//! The derived key itself never reaches this module: `key_ref` is the opaque `vault://`
//! reference the secret store minted, exactly like every other credential column. The
//! destinations, the ledger of written archives and the verifications are in
//! `backup_ledger_store` (RD-160-02).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{BackupOrigin, BackupRunState};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

/// How many finished runs the history keeps; older ones are dropped when a run finishes.
pub const BACKUP_RUNS_KEPT: i64 = 100;

/// Where backups are written: `local`, `object_storage` or `rclone` (RD-160-02), each with its
/// own retention.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupDestinationRecord {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    /// The newest archives kept; `None` is no limit by count.
    pub keep_last: Option<u32>,
    /// The days archives are kept; `None` is no limit by age.
    pub keep_days: Option<u32>,
    pub created_at: DateTime<Utc>,
}

/// The key a backup is sealed with, as the store knows it: a reference, never the bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupKeyRecord {
    /// The secret store's reference to the derived key.
    pub reference: String,
    /// Base64 of the Argon2id salt the key was derived with.
    pub salt: String,
    pub fingerprint: String,
    pub set_at: DateTime<Utc>,
}

/// The one backup configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupConfig {
    pub enabled: bool,
    pub schedule: String,
    pub timezone: String,
    /// Every destination, oldest first; each receives its own copy of every archive.
    pub destinations: Vec<BackupDestinationRecord>,
    pub key: Option<BackupKeyRecord>,
    pub next_run_at: Option<DateTime<Utc>>,
    /// This installation's id, part of every archive name (RD-160-02).
    pub instance_id: String,
    /// The scheduled verification's cron expression, read in `timezone`; `None` is off.
    pub verify_schedule: Option<String>,
    pub verify_next_run_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// A destination as it is created or replaced.
#[derive(Clone, Debug)]
pub struct NewBackupDestination {
    pub kind: String,
    pub name: String,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub keep_last: Option<u32>,
    pub keep_days: Option<u32>,
}

/// Everything a schedule save writes at once. The destinations have their own calls.
#[derive(Clone, Debug)]
pub struct BackupConfigUpdate {
    pub enabled: bool,
    pub schedule: String,
    pub timezone: String,
    pub next_run_at: Option<DateTime<Utc>>,
    pub verify_schedule: Option<String>,
    pub verify_next_run_at: Option<DateTime<Utc>>,
}

/// The start of a run.
#[derive(Clone, Debug)]
pub struct NewBackupRun {
    pub id: String,
    pub origin: BackupOrigin,
    pub started_at: DateTime<Utc>,
    pub destination_id: Option<String>,
    pub destination: Option<String>,
}

/// How a run ended.
#[derive(Clone, Debug)]
pub enum BackupRunOutcome {
    /// At least one destination has the archive. `error_code` and `error_detail` name the
    /// first destination that does not, when one does not (RD-160-02).
    Succeeded {
        archive_name: String,
        size_bytes: u64,
        sha256: String,
        parts: serde_json::Value,
        error_code: Option<String>,
        error_detail: Option<String>,
    },
    Failed {
        code: String,
        detail: String,
    },
}

/// One row of the history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupRun {
    pub id: String,
    pub origin: BackupOrigin,
    pub state: BackupRunState,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub destination: Option<String>,
    pub archive_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
    pub parts: Option<serde_json::Value>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    /// How each destination fared (RD-160-02).
    pub destinations: Vec<crate::BackupRunDestination>,
}

/// The code an interrupted run is recorded with.
pub const BACKUP_INTERRUPTED: &str = "backup.interrupted";

#[derive(FromRow)]
struct ConfigRow {
    enabled: bool,
    schedule: String,
    timezone: String,
    key_ref: Option<String>,
    key_salt: Option<String>,
    key_fingerprint: Option<String>,
    key_set_at: Option<DateTime<Utc>>,
    next_run_at: Option<DateTime<Utc>>,
    instance_id: Option<String>,
    verify_schedule: Option<String>,
    verify_next_run_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct RunRow {
    id: String,
    origin: String,
    state: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    destination: Option<String>,
    archive_name: Option<String>,
    size_bytes: Option<i64>,
    sha256: Option<String>,
    parts_json: Option<String>,
    error_code: Option<String>,
    error_detail: Option<String>,
}

impl TryFrom<RunRow> for BackupRun {
    type Error = anyhow::Error;

    fn try_from(row: RunRow) -> Result<Self> {
        Ok(Self {
            origin: BackupOrigin::parse(&row.origin)
                .with_context(|| format!("unknown backup origin {}", row.origin))?,
            state: BackupRunState::parse(&row.state)
                .with_context(|| format!("unknown backup state {}", row.state))?,
            id: row.id,
            started_at: row.started_at,
            finished_at: row.finished_at,
            destination: row.destination,
            archive_name: row.archive_name,
            size_bytes: row.size_bytes.and_then(|size| u64::try_from(size).ok()),
            sha256: row.sha256,
            parts: row
                .parts_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .context("parse stored backup parts")?,
            error_code: row.error_code,
            error_detail: row.error_detail,
            destinations: Vec::new(),
        })
    }
}

const RUN_COLUMNS: &str = "id, origin, state, started_at, finished_at, destination, \
     archive_name, size_bytes, sha256, parts_json, error_code, error_detail";

pub(crate) async fn config(pool: &SqlitePool) -> Result<BackupConfig> {
    let row = sqlx::query_as::<_, ConfigRow>(
        "SELECT enabled, schedule, timezone, key_ref, key_salt, key_fingerprint, key_set_at, \
         next_run_at, instance_id, verify_schedule, verify_next_run_at, updated_at \
         FROM backup_config WHERE id = 1",
    )
    .fetch_one(pool)
    .await
    .context("read backup configuration")?;
    let destinations = crate::backup_ledger_store::destinations(pool).await?;
    let key = match (row.key_ref, row.key_salt) {
        (Some(reference), Some(salt)) => Some(BackupKeyRecord {
            reference,
            salt,
            fingerprint: row.key_fingerprint.unwrap_or_default(),
            set_at: row.key_set_at.unwrap_or(row.updated_at),
        }),
        _ => None,
    };
    Ok(BackupConfig {
        enabled: row.enabled,
        schedule: row.schedule,
        timezone: row.timezone,
        destinations,
        key,
        next_run_at: row.next_run_at,
        instance_id: row.instance_id.unwrap_or_default(),
        verify_schedule: row.verify_schedule,
        verify_next_run_at: row.verify_next_run_at,
        updated_at: row.updated_at,
    })
}

pub(crate) async fn runs(pool: &SqlitePool, limit: u32) -> Result<Vec<BackupRun>> {
    let rows = sqlx::query_as::<_, RunRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLUMNS} FROM backup_runs ORDER BY started_at DESC, id LIMIT ?"
    )))
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?;
    let mut runs = Vec::with_capacity(rows.len());
    for row in rows {
        runs.push(with_destinations(pool, row.try_into()?).await?);
    }
    Ok(runs)
}

pub(crate) async fn run(pool: &SqlitePool, id: &str) -> Result<Option<BackupRun>> {
    let row = sqlx::query_as::<_, RunRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {RUN_COLUMNS} FROM backup_runs WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(with_destinations(pool, row.try_into()?).await?)),
        None => Ok(None),
    }
}

async fn with_destinations(pool: &SqlitePool, mut run: BackupRun) -> Result<BackupRun> {
    run.destinations = crate::backup_ledger_store::run_destinations(pool, &run.id).await?;
    Ok(run)
}

/// Saves the schedule and the verification schedule.
pub(crate) async fn save_config(
    connection: &mut SqliteConnection,
    update: BackupConfigUpdate,
) -> Result<()> {
    sqlx::query(
        "UPDATE backup_config SET enabled = ?, schedule = ?, timezone = ?, next_run_at = ?, \
         verify_schedule = ?, verify_next_run_at = ?, updated_at = ? WHERE id = 1",
    )
    .bind(update.enabled)
    .bind(&update.schedule)
    .bind(&update.timezone)
    .bind(update.next_run_at)
    .bind(&update.verify_schedule)
    .bind(update.verify_next_run_at)
    .bind(Utc::now())
    .execute(connection)
    .await?;
    Ok(())
}

/// Replaces the key reference; returns the reference it replaced, for the caller to remove
/// from the secret store once the new one is committed.
pub(crate) async fn set_key(
    connection: &mut SqliteConnection,
    key: BackupKeyRecord,
) -> Result<Option<String>> {
    let mut tx = connection.begin().await?;
    let previous: Option<String> =
        sqlx::query_scalar("SELECT key_ref FROM backup_config WHERE id = 1")
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        "UPDATE backup_config SET key_ref = ?, key_salt = ?, key_fingerprint = ?, \
         key_set_at = ?, updated_at = ? WHERE id = 1",
    )
    .bind(&key.reference)
    .bind(&key.salt)
    .bind(&key.fingerprint)
    .bind(key.set_at)
    .bind(Utc::now())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(previous.filter(|reference| *reference != key.reference))
}

/// Sets when the schedule is next due, leaving everything else alone.
pub(crate) async fn arm(
    connection: &mut SqliteConnection,
    next_run_at: Option<DateTime<Utc>>,
) -> Result<()> {
    sqlx::query("UPDATE backup_config SET next_run_at = ? WHERE id = 1")
        .bind(next_run_at)
        .execute(connection)
        .await?;
    Ok(())
}

/// Records the start of a run; `false` when another run is still running.
pub(crate) async fn begin_run(
    connection: &mut SqliteConnection,
    run: NewBackupRun,
) -> Result<bool> {
    let inserted = sqlx::query(
        "INSERT INTO backup_runs (id, origin, state, started_at, destination_id, destination) \
         VALUES (?, ?, 'running', ?, ?, ?)",
    )
    .bind(&run.id)
    .bind(run.origin.as_str())
    .bind(run.started_at)
    .bind(&run.destination_id)
    .bind(&run.destination)
    .execute(connection)
    .await;
    match inserted {
        Ok(_) => Ok(true),
        Err(error)
            if error.as_database_error().is_some_and(|database| {
                matches!(database.kind(), sqlx::error::ErrorKind::UniqueViolation)
            }) =>
        {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

/// Records how a run ended and drops the history beyond [`BACKUP_RUNS_KEPT`].
pub(crate) async fn finish_run(
    connection: &mut SqliteConnection,
    id: &str,
    outcome: BackupRunOutcome,
) -> Result<()> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    match outcome {
        BackupRunOutcome::Succeeded {
            archive_name,
            size_bytes,
            sha256,
            parts,
            error_code,
            error_detail,
        } => {
            sqlx::query(
                "UPDATE backup_runs SET state = 'succeeded', finished_at = ?, archive_name = ?, \
                 size_bytes = ?, sha256 = ?, parts_json = ?, error_code = ?, error_detail = ? \
                 WHERE id = ? AND state = 'running'",
            )
            .bind(now)
            .bind(archive_name)
            .bind(i64::try_from(size_bytes).unwrap_or(i64::MAX))
            .bind(sha256)
            .bind(serde_json::to_string(&parts)?)
            .bind(error_code)
            .bind(error_detail)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        BackupRunOutcome::Failed { code, detail } => {
            sqlx::query(
                "UPDATE backup_runs SET state = 'failed', finished_at = ?, error_code = ?, \
                 error_detail = ? WHERE id = ? AND state = 'running'",
            )
            .bind(now)
            .bind(code)
            .bind(detail)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
    }
    prune(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Marks every run still `running` as interrupted — its destinations and any verification
/// still running too (RD-160-02); called once when the process starts, when no run of this
/// process can exist yet. Returns the runs marked.
pub(crate) async fn interrupt_runs(connection: &mut SqliteConnection) -> Result<u64> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE backup_runs SET state = 'interrupted', finished_at = ?, error_code = ?, \
         error_detail = 'the service stopped while the backup was being written' \
         WHERE state = 'running'",
    )
    .bind(now)
    .bind(BACKUP_INTERRUPTED)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE backup_run_destinations SET state = 'interrupted', finished_at = ?, \
         error_code = ? WHERE state = 'running'",
    )
    .bind(now)
    .bind(BACKUP_INTERRUPTED)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE backup_verifications SET state = 'interrupted', finished_at = ?, \
         error_code = ? WHERE state = 'running'",
    )
    .bind(now)
    .bind(BACKUP_INTERRUPTED)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected())
}

async fn prune(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    sqlx::query(
        "DELETE FROM backup_runs WHERE state != 'running' AND id NOT IN \
         (SELECT id FROM backup_runs ORDER BY started_at DESC, id LIMIT ?)",
    )
    .bind(BACKUP_RUNS_KEPT)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
