//! Reads and edits of NZB imports, and the row type they are read through.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{ByteCount, EventEnvelope, EventKind, NzbImport, NzbImportId, NzbImportState};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use super::NzbImportChange;
use crate::{enum_string, error::StoreError, parse_id, writer::insert_event};

pub(crate) async fn list_imports(pool: &SqlitePool) -> Result<Vec<NzbImport>> {
    // The LinkGrabber's manual order is one sequence over both tables, so this list has to come
    // out of the database in it; sorting by creation time in the client is what made an import
    // un-draggable in the first place.
    sqlx::query_as::<_, NzbImportRow>(sqlx::AssertSqlSafe(format!(
        "{IMPORT_SELECT} ORDER BY position ASC, created_at ASC"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn update_import(
    connection: &mut SqliteConnection,
    id: NzbImportId,
    change: NzbImportChange,
) -> Result<(NzbImport, EventEnvelope)> {
    anyhow::ensure!(
        change.category_id.is_some() || change.priority.is_some(),
        "no NZB import change specified"
    );
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "nzb_import_id": id, "updated": true }),
    );
    let mut transaction = connection.begin().await?;
    let state: String = sqlx::query_scalar("SELECT state FROM nzb_imports WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    anyhow::ensure!(
        state != "enqueued",
        StoreError::wrong_state("enqueued NZB import cannot be changed")
    );

    if let Some(category_id) = change.category_id {
        sqlx::query("UPDATE nzb_imports SET category_id = ?, updated_at = ? WHERE id = ?")
            .bind(category_id.map(|value| value.to_string()))
            .bind(event.occurred_at)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
    }
    if let Some(priority) = change.priority {
        sqlx::query("UPDATE nzb_imports SET priority = ?, updated_at = ? WHERE id = ?")
            .bind(i64::from(priority.as_i32()))
            .bind(event.occurred_at)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
    }
    let updated = get_by_id_connection(&mut transaction, id)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((updated, event))
}

/// One import by id, or `None`.
pub(crate) async fn get_import(pool: &SqlitePool, id: NzbImportId) -> Result<Option<NzbImport>> {
    sqlx::query_as::<_, NzbImportRow>(sqlx::AssertSqlSafe(format!("{IMPORT_SELECT} WHERE id = ?")))
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

/// Marks an import as handed to `remote_job_id` (RD-191-13): one still in the LinkGrabber
/// (`expected` = `Imported`), or the one behind a queued package (`Enqueued`), which the
/// Downloads view hands over.
///
/// The import is not consumed: it stays reviewable, and enqueueing it stays possible, because
/// a hand-over the provider then refuses must not cost the person their NZB.
pub(crate) async fn mark_remote_job(
    connection: &mut SqliteConnection,
    id: NzbImportId,
    remote_job_id: rd_core::RemoteJobId,
    expected: NzbImportState,
) -> Result<(NzbImport, EventEnvelope)> {
    let event = EventEnvelope::new(
        EventKind::CollectorChanged,
        serde_json::json!({ "nzb_import_id": id, "updated": true }),
    );
    let mut transaction = connection.begin().await?;
    let state: String = sqlx::query_scalar("SELECT state FROM nzb_imports WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *transaction)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    anyhow::ensure!(
        state == enum_string(expected)?,
        StoreError::wrong_state("the NZB import is no longer where it was handed over from")
    );
    sqlx::query("UPDATE nzb_imports SET remote_job_id = ?, updated_at = ? WHERE id = ?")
        .bind(remote_job_id.to_string())
        .bind(event.occurred_at)
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    let updated = get_by_id_connection(&mut transaction, id)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((updated, event))
}

pub(crate) async fn delete_import(
    connection: &mut SqliteConnection,
    id: NzbImportId,
) -> Result<EventEnvelope> {
    sqlx::query_scalar::<_, String>("SELECT state FROM nzb_imports WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *connection)
        .await?
        .context(StoreError::not_found("NZB import not found"))?;
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM packages WHERE nzb_import_id = ?")
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?;
    anyhow::ensure!(
        queued == 0,
        StoreError::wrong_state(
            "active NZB import cannot be removed (delete the package in the downloader instead)"
        )
    );
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "nzb_import_id": id, "removed": true }),
    );
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM nzb_imports WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// Drops a completed package's NZB import history while keeping the package itself
/// (`packages.nzb_import_id` is `ON DELETE SET NULL`, files/segments cascade).
/// No-op for packages without an import link.
pub(crate) async fn forget_import_for_package(
    connection: &mut SqliteConnection,
    package_id: rd_core::PackageId,
) -> Result<Option<EventEnvelope>> {
    let import_id: Option<String> =
        sqlx::query_scalar("SELECT nzb_import_id FROM packages WHERE id = ?")
            .bind(package_id.to_string())
            .fetch_optional(&mut *connection)
            .await?
            .flatten();
    let Some(import_id) = import_id else {
        return Ok(None);
    };
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "nzb_import_id": import_id, "removed": true }),
    );
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM nzb_imports WHERE id = ?")
        .bind(&import_id)
        .execute(&mut *transaction)
        .await?;
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(Some(event))
}

pub(super) async fn get_by_hash_connection(
    connection: &mut SqliteConnection,
    sha256: &str,
) -> Result<Option<NzbImport>> {
    sqlx::query_as::<_, NzbImportRow>(sqlx::AssertSqlSafe(format!(
        "{IMPORT_SELECT} WHERE sha256 = ?"
    )))
    .bind(sha256)
    .fetch_optional(connection)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(super) async fn get_by_id_connection(
    connection: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: NzbImportId,
) -> Result<Option<NzbImport>> {
    sqlx::query_as::<_, NzbImportRow>(sqlx::AssertSqlSafe(format!("{IMPORT_SELECT} WHERE id = ?")))
        .bind(id.to_string())
        .fetch_optional(&mut **connection)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

// The hand-over's account is read through the job rather than stored twice: the job's row is
// what the mark stands for, and `ON DELETE SET NULL` clears `remote_job_id` with it (RD-191-13).
const IMPORT_SELECT: &str = "SELECT id, name, sha256, state, file_count, segment_count, total_bytes, category_id, priority, import_mode, source_path, last_error, password_ref IS NOT NULL AS has_password, position, remote_job_id, (SELECT account_id FROM remote_jobs WHERE remote_jobs.id = nzb_imports.remote_job_id) AS remote_account_id, created_at FROM nzb_imports";

#[derive(FromRow)]
struct NzbImportRow {
    id: String,
    name: String,
    sha256: String,
    state: String,
    file_count: i64,
    segment_count: i64,
    total_bytes: i64,
    category_id: Option<String>,
    priority: Option<i64>,
    import_mode: String,
    source_path: Option<String>,
    last_error: Option<String>,
    has_password: i64,
    position: i64,
    remote_job_id: Option<String>,
    remote_account_id: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<NzbImportRow> for NzbImport {
    type Error = anyhow::Error;

    fn try_from(row: NzbImportRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            sha256: row.sha256,
            state: serde_json::from_str(&format!("\"{}\"", row.state))?,
            file_count: u32::try_from(row.file_count)?,
            segment_count: u32::try_from(row.segment_count)?,
            total_bytes: ByteCount::new(u64::try_from(row.total_bytes)?)
                .map_err(anyhow::Error::msg)?,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
            priority: row
                .priority
                .map(|value| rd_core::DownloadPriority::from_i32(value as i32)),
            import_mode: serde_json::from_str(&format!("\"{}\"", row.import_mode))?,
            source_path: row.source_path,
            error: row.last_error,
            duplicate: false,
            has_password: row.has_password != 0,
            // In the vault (RD-190-04); revealed for the answers that show it.
            password: None,
            position: row.position,
            handed_over: match (row.remote_job_id, row.remote_account_id) {
                (Some(job), Some(account)) => Some(rd_core::NzbHandOver {
                    remote_job_id: parse_id(&job)?,
                    account_id: parse_id(&account)?,
                }),
                _ => None,
            },
            created_at: row.created_at,
        })
    }
}
