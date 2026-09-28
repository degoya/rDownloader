//! The history of verified moves and dedupe links (RD-150-02). See
//! `migrations/0100_storage_operations.sql`.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rd_core::{DownloadId, PackageId, StorageOperationKind, StorageOperationState};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::parse_id;

/// Rows kept; the oldest beyond this go when a new one is started.
pub const STORAGE_OPERATIONS_KEPT: i64 = 2000;

/// What starting an operation records.
#[derive(Clone, Debug)]
pub struct NewStorageOperation {
    pub kind: StorageOperationKind,
    pub package_id: Option<PackageId>,
    pub download_id: Option<DownloadId>,
    pub source_path: String,
    pub target_path: String,
    pub size_bytes: Option<u64>,
}

/// How an operation ended.
#[derive(Clone, Debug)]
pub struct StorageOperationOutcome {
    pub state: StorageOperationState,
    /// Where the data ended up, when that differs from the target the operation started with
    /// (a taken name moves the file beside it).
    pub target_path: Option<String>,
    pub size_bytes: Option<u64>,
    /// The digest both copies were verified to share, when the operation compared them.
    pub verified_digest: Option<String>,
    /// A stable code for a failure, translated in the interface.
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

impl StorageOperationOutcome {
    /// A successful end, with what was verified on the way.
    #[must_use]
    pub fn completed(size_bytes: Option<u64>, verified_digest: Option<String>) -> Self {
        Self {
            state: StorageOperationState::Completed,
            target_path: None,
            size_bytes,
            verified_digest,
            error_code: None,
            error_message: None,
        }
    }

    /// A failure with its code and the reason as the error said it.
    #[must_use]
    pub fn failed(code: &str, message: String) -> Self {
        Self {
            state: StorageOperationState::Failed,
            target_path: None,
            size_bytes: None,
            verified_digest: None,
            error_code: Some(code.to_owned()),
            error_message: Some(message),
        }
    }
}

/// One stored operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageOperation {
    pub id: i64,
    pub kind: StorageOperationKind,
    pub state: StorageOperationState,
    pub package_id: Option<PackageId>,
    pub download_id: Option<DownloadId>,
    pub source_path: String,
    pub target_path: String,
    pub size_bytes: Option<u64>,
    pub verified_digest: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct Row {
    id: i64,
    kind: String,
    state: String,
    package_id: Option<String>,
    download_id: Option<String>,
    source_path: String,
    target_path: String,
    size_bytes: Option<i64>,
    verified_digest: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
    started_at: String,
    finished_at: Option<String>,
}

fn timestamp(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .context("parse stored timestamp")?
        .with_timezone(&Utc))
}

fn bytes(value: Option<u64>) -> Option<i64> {
    value.map(|value| i64::try_from(value).unwrap_or(i64::MAX))
}

impl TryFrom<Row> for StorageOperation {
    type Error = anyhow::Error;

    fn try_from(row: Row) -> Result<Self> {
        Ok(Self {
            id: row.id,
            kind: StorageOperationKind::parse(&row.kind).with_context(|| {
                format!("storage operation {} names kind {:?}", row.id, row.kind)
            })?,
            state: StorageOperationState::parse(&row.state).with_context(|| {
                format!("storage operation {} names state {:?}", row.id, row.state)
            })?,
            package_id: row.package_id.as_deref().map(parse_id).transpose()?,
            download_id: row.download_id.as_deref().map(parse_id).transpose()?,
            source_path: row.source_path,
            target_path: row.target_path,
            size_bytes: row.size_bytes.and_then(|value| u64::try_from(value).ok()),
            verified_digest: row.verified_digest,
            error_code: row.error_code,
            error_message: row.error_message,
            started_at: parse_time(&row.started_at)?,
            finished_at: row.finished_at.as_deref().map(parse_time).transpose()?,
        })
    }
}

const COLUMNS: &str = "id, kind, state, package_id, download_id, source_path, target_path, \
     size_bytes, verified_digest, error_code, error_message, started_at, finished_at";

/// Newest first.
pub(crate) async fn list_storage_operations(
    pool: &SqlitePool,
    limit: u32,
) -> Result<Vec<StorageOperation>> {
    sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM storage_operations ORDER BY id DESC LIMIT ?"
    )))
    .bind(i64::from(limit))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(StorageOperation::try_from)
    .collect()
}

/// Records a starting operation and answers its id; drops the oldest rows beyond the cap.
pub(crate) async fn start_storage_operation(
    connection: &mut SqliteConnection,
    operation: NewStorageOperation,
) -> Result<i64> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO storage_operations \
           (kind, state, package_id, download_id, source_path, target_path, size_bytes, \
            started_at) \
         VALUES (?, 'running', ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(operation.kind.as_str())
    .bind(operation.package_id.map(|id| id.to_string()))
    .bind(operation.download_id.map(|id| id.to_string()))
    .bind(&operation.source_path)
    .bind(&operation.target_path)
    .bind(bytes(operation.size_bytes))
    .bind(timestamp(&Utc::now()))
    .fetch_one(&mut *connection)
    .await?;
    sqlx::query("DELETE FROM storage_operations WHERE id <= ?")
        .bind(id - STORAGE_OPERATIONS_KEPT)
        .execute(&mut *connection)
        .await?;
    Ok(id)
}

/// Records how an operation ended. Only a row that is still running is changed, so a late
/// answer cannot rewrite a history that recovery already settled.
pub(crate) async fn finish_storage_operation(
    connection: &mut SqliteConnection,
    id: i64,
    outcome: StorageOperationOutcome,
) -> Result<()> {
    sqlx::query(
        "UPDATE storage_operations SET state = ?, target_path = COALESCE(?, target_path), \
           size_bytes = COALESCE(?, size_bytes), verified_digest = ?, error_code = ?, \
           error_message = ?, finished_at = ? \
         WHERE id = ? AND state = 'running'",
    )
    .bind(outcome.state.as_str())
    .bind(outcome.target_path)
    .bind(bytes(outcome.size_bytes))
    .bind(outcome.verified_digest)
    .bind(outcome.error_code)
    .bind(outcome.error_message)
    .bind(timestamp(&Utc::now()))
    .bind(id)
    .execute(connection)
    .await?;
    Ok(())
}

/// Marks every row a stopped process left `running` as `interrupted`. Run once at start,
/// before anything can start a new operation.
pub(crate) async fn interrupt_running_storage_operations(
    connection: &mut SqliteConnection,
) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE storage_operations SET state = 'interrupted', finished_at = ? \
         WHERE state = 'running'",
    )
    .bind(timestamp(&Utc::now()))
    .execute(connection)
    .await?;
    Ok(result.rows_affected())
}

/// Packages whose category change still has data to carry over.
pub(crate) async fn packages_with_outstanding_move(pool: &SqlitePool) -> Result<Vec<PackageId>> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM packages WHERE previous_destination IS NOT NULL ORDER BY id",
    )
    .fetch_all(pool)
    .await?
    .iter()
    .map(|id| parse_id(id))
    .collect()
}
