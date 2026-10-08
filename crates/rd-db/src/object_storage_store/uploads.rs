//! The multipart uploads that survive a restart (RD-150-04): one record per destination object,
//! one row per confirmed part.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::ObjectStorageProfileId;
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

/// One multipart (or single-request) upload of one local file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectUpload {
    pub id: String,
    pub profile_id: ObjectStorageProfileId,
    pub owner: String,
    pub bucket: String,
    pub object_key: String,
    pub local_path: String,
    pub local_size: u64,
    pub local_modified: Option<String>,
    pub part_size: u64,
    /// The service's multipart id; `None` for a file that goes up in one request.
    pub upload_id: Option<String>,
    pub checksums: bool,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// The parts the service confirmed, by part number.
    pub parts: Vec<ObjectUploadPart>,
}

/// One confirmed part.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectUploadPart {
    /// Zero-based, the way the upload counts; the service's part number is one higher.
    pub part_number: u32,
    /// What the completion names the part by: the ETag, with the checksums when they were sent.
    pub content_id: String,
    pub size: u64,
}

const UPLOAD_COLUMNS: &str = "id, profile_id, owner, bucket, object_key, local_path, local_size, \
     local_modified, part_size, upload_id, checksums, completed_at, created_at";

/// The upload recorded for one destination object, with its confirmed parts.
pub(crate) async fn upload(
    pool: &SqlitePool,
    profile_id: ObjectStorageProfileId,
    bucket: &str,
    object_key: &str,
) -> Result<Option<ObjectUpload>> {
    let row = sqlx::query_as::<_, UploadRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {UPLOAD_COLUMNS} FROM object_uploads \
         WHERE profile_id = ? AND bucket = ? AND object_key = ?"
    )))
    .bind(profile_id.to_string())
    .bind(bucket)
    .bind(object_key)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(row) => Ok(Some(with_parts(pool, row).await?)),
        None => Ok(None),
    }
}

/// Uploads recorded by one profile, or all of them started before `before` when no profile
/// is named — the two questions the abort sweep asks.
pub(crate) async fn uploads(
    pool: &SqlitePool,
    profile_id: Option<ObjectStorageProfileId>,
    before: Option<DateTime<Utc>>,
) -> Result<Vec<ObjectUpload>> {
    let rows = sqlx::query_as::<_, UploadRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {UPLOAD_COLUMNS} FROM object_uploads \
         WHERE (?1 IS NULL OR profile_id = ?1) AND (?2 IS NULL OR created_at < ?2) \
         ORDER BY created_at"
    )))
    .bind(profile_id.map(|id| id.to_string()))
    .bind(before)
    .fetch_all(pool)
    .await?;
    let mut uploads = Vec::with_capacity(rows.len());
    for row in rows {
        uploads.push(with_parts(pool, row).await?);
    }
    Ok(uploads)
}

async fn with_parts(pool: &SqlitePool, row: UploadRow) -> Result<ObjectUpload> {
    let parts = sqlx::query_as::<_, PartRow>(
        "SELECT part_number, content_id, size FROM object_upload_parts \
         WHERE upload_id = ? ORDER BY part_number",
    )
    .bind(&row.id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|part| {
        Ok(ObjectUploadPart {
            part_number: u32::try_from(part.part_number).context("part number out of range")?,
            content_id: part.content_id,
            size: u64::try_from(part.size).context("part size out of range")?,
        })
    })
    .collect::<Result<Vec<_>>>()?;
    let mut upload: ObjectUpload = row.try_into()?;
    upload.parts = parts;
    Ok(upload)
}

/// Records an upload that is about to start, replacing whatever was recorded for the same
/// destination before — together with its parts, which belonged to an upload the caller has
/// already aborted.
pub(crate) async fn begin_upload(
    connection: &mut SqliteConnection,
    upload: ObjectUpload,
) -> Result<()> {
    let mut tx = connection.begin().await?;
    sqlx::query(
        "DELETE FROM object_uploads WHERE profile_id = ? AND bucket = ? AND object_key = ?",
    )
    .bind(upload.profile_id.to_string())
    .bind(&upload.bucket)
    .bind(&upload.object_key)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO object_uploads (id, profile_id, owner, bucket, object_key, local_path, \
         local_size, local_modified, part_size, upload_id, checksums, completed_at, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&upload.id)
    .bind(upload.profile_id.to_string())
    .bind(&upload.owner)
    .bind(&upload.bucket)
    .bind(&upload.object_key)
    .bind(&upload.local_path)
    .bind(i64::try_from(upload.local_size).context("file size out of range")?)
    .bind(&upload.local_modified)
    .bind(i64::try_from(upload.part_size).context("part size out of range")?)
    .bind(&upload.upload_id)
    .bind(upload.checksums)
    .bind(upload.completed_at)
    .bind(upload.created_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Records one part the service confirmed.
pub(crate) async fn record_part(
    connection: &mut SqliteConnection,
    upload_id: &str,
    part: ObjectUploadPart,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO object_upload_parts (upload_id, part_number, content_id, size) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(upload_id, part_number) DO UPDATE SET \
         content_id = excluded.content_id, size = excluded.size",
    )
    .bind(upload_id)
    .bind(i64::from(part.part_number))
    .bind(&part.content_id)
    .bind(i64::try_from(part.size).context("part size out of range")?)
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Marks an upload finished and drops its parts, which nothing needs any more.
pub(crate) async fn complete_upload(connection: &mut SqliteConnection, id: &str) -> Result<()> {
    let mut tx = connection.begin().await?;
    sqlx::query("UPDATE object_uploads SET completed_at = ?, upload_id = NULL WHERE id = ?")
        .bind(Utc::now())
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM object_upload_parts WHERE upload_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Forgets one upload record, or every record of an owner.
pub(crate) async fn forget_uploads(
    connection: &mut SqliteConnection,
    id: Option<&str>,
    owner: Option<&str>,
) -> Result<u64> {
    // The last clause keeps a call that names neither from emptying the table.
    let result = sqlx::query(
        "DELETE FROM object_uploads WHERE (?1 IS NULL OR id = ?1) \
         AND (?2 IS NULL OR owner = ?2) AND (?1 IS NOT NULL OR ?2 IS NOT NULL)",
    )
    .bind(id)
    .bind(owner)
    .execute(&mut *connection)
    .await?;
    Ok(result.rows_affected())
}

#[derive(FromRow)]
struct UploadRow {
    id: String,
    profile_id: String,
    owner: String,
    bucket: String,
    object_key: String,
    local_path: String,
    local_size: i64,
    local_modified: Option<String>,
    part_size: i64,
    upload_id: Option<String>,
    checksums: bool,
    completed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl TryFrom<UploadRow> for ObjectUpload {
    type Error = anyhow::Error;

    fn try_from(row: UploadRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            profile_id: row.profile_id.parse()?,
            owner: row.owner,
            bucket: row.bucket,
            object_key: row.object_key,
            local_path: row.local_path,
            local_size: u64::try_from(row.local_size).context("file size out of range")?,
            local_modified: row.local_modified,
            part_size: u64::try_from(row.part_size).context("part size out of range")?,
            upload_id: row.upload_id,
            checksums: row.checksums,
            completed_at: row.completed_at,
            created_at: row.created_at,
            parts: Vec::new(),
        })
    }
}

#[derive(FromRow)]
struct PartRow {
    part_number: i64,
    content_id: String,
    size: i64,
}
