//! The backup verifications: their records, how each ended, the history and the schedule's
//! next due time (RD-160-02).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{BackupOrigin, BackupVerifyState};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use super::BACKUP_VERIFICATIONS_KEPT;

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
