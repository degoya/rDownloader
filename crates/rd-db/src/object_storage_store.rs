//! Persistence of object storage profiles and of the multipart uploads that survive a restart
//! (RD-150-04).
//!
//! Credential values never reach this module; it stores the opaque `vault://` references the
//! secret store minted, exactly like [`crate::remote_store`].

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{EventEnvelope, EventKind, ObjectStorageProfile, ObjectStorageProfileId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{enum_string, error::StoreError, parse_enum, writer::insert_event};

/// Editable profile fields, for a new profile and for an update alike. Secret references are
/// replaced wholesale on update; the caller cleans up the ones that fall out of use.
#[derive(Clone, Debug)]
pub struct NewObjectStorageProfile {
    pub name: String,
    pub provider: rd_core::ObjectStorageProvider,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub addressing: rd_core::ObjectAddressing,
    pub credential_source: rd_core::ObjectCredentialSource,
    pub access_key_id: Option<String>,
    pub account: Option<String>,
    pub secret_ref: Option<String>,
    pub session_token_ref: Option<String>,
    pub checksums: bool,
    pub enabled: bool,
}

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

const COLUMNS: &str = "id, name, provider, endpoint, region, bucket, addressing, \
     credential_source, access_key_id, account, secret_ref, session_token_ref, checksums, \
     enabled, created_at, updated_at";

const UPLOAD_COLUMNS: &str = "id, profile_id, owner, bucket, object_key, local_path, local_size, \
     local_modified, part_size, upload_id, checksums, completed_at, created_at";

fn changed_event() -> EventEnvelope {
    // The same event the remote logins raise: both belong to the transfer settings page, and
    // a client that reloads one list on it reloads the other.
    EventEnvelope::new(
        EventKind::RemoteCredentialChanged,
        serde_json::json!({ "resource": "object_storage_profile" }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<ObjectStorageProfile>> {
    sqlx::query_as::<_, ProfileRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM object_storage_profiles ORDER BY name"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn get(
    pool: &SqlitePool,
    id: ObjectStorageProfileId,
) -> Result<Option<ObjectStorageProfile>> {
    sqlx::query_as::<_, ProfileRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM object_storage_profiles WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

const DUPLICATE: &str = "an object storage profile with this name already exists";

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewObjectStorageProfile,
) -> Result<(ObjectStorageProfile, EventEnvelope)> {
    let now = Utc::now();
    let id = ObjectStorageProfileId::new();
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO object_storage_profiles (id, name, provider, endpoint, region, bucket, \
         addressing, credential_source, access_key_id, account, secret_ref, session_token_ref, \
         checksums, enabled, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&input.name)
    .bind(enum_string(input.provider)?)
    .bind(&input.endpoint)
    .bind(&input.region)
    .bind(&input.bucket)
    .bind(enum_string(input.addressing)?)
    .bind(enum_string(input.credential_source)?)
    .bind(&input.access_key_id)
    .bind(&input.account)
    .bind(&input.secret_ref)
    .bind(&input.session_token_ref)
    .bind(input.checksums)
    .bind(input.enabled)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|error| crate::error::tag_duplicate(error, DUPLICATE))?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let value = fetch(connection, id).await?;
    Ok((value, event))
}

/// Applies an update and returns the profile plus the references it no longer uses.
pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: ObjectStorageProfileId,
    input: NewObjectStorageProfile,
) -> Result<(ObjectStorageProfile, Vec<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let previous = sqlx::query_as::<_, ProfileRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM object_storage_profiles WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(&mut *tx)
    .await?
    .context(StoreError::not_found("object storage profile not found"))?;
    sqlx::query(
        "UPDATE object_storage_profiles SET name = ?, provider = ?, endpoint = ?, region = ?, \
         bucket = ?, addressing = ?, credential_source = ?, access_key_id = ?, account = ?, \
         secret_ref = ?, session_token_ref = ?, checksums = ?, enabled = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(&input.name)
    .bind(enum_string(input.provider)?)
    .bind(&input.endpoint)
    .bind(&input.region)
    .bind(&input.bucket)
    .bind(enum_string(input.addressing)?)
    .bind(enum_string(input.credential_source)?)
    .bind(&input.access_key_id)
    .bind(&input.account)
    .bind(&input.secret_ref)
    .bind(&input.session_token_ref)
    .bind(input.checksums)
    .bind(input.enabled)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| crate::error::tag_duplicate(error, DUPLICATE))?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let orphaned = [
        (previous.secret_ref, &input.secret_ref),
        (previous.session_token_ref, &input.session_token_ref),
    ]
    .into_iter()
    .filter_map(|(old, new)| old.filter(|old| Some(old) != new.as_ref()))
    .collect();
    let value = fetch(connection, id).await?;
    Ok((value, orphaned, event))
}

/// Deletes a profile with the upload records that belong to it and returns the secret
/// references left behind. Aborting those uploads at the service is the caller's job, done
/// before this while the credentials still exist.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: ObjectStorageProfileId,
) -> Result<(Vec<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let existing = sqlx::query_as::<_, ProfileRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM object_storage_profiles WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(&mut *tx)
    .await?
    .context(StoreError::not_found("object storage profile not found"))?;
    sqlx::query("DELETE FROM object_storage_profiles WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let orphaned = [existing.secret_ref, existing.session_token_ref]
        .into_iter()
        .flatten()
        .collect();
    Ok((orphaned, event))
}

async fn fetch(
    connection: &mut SqliteConnection,
    id: ObjectStorageProfileId,
) -> Result<ObjectStorageProfile> {
    sqlx::query_as::<_, ProfileRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM object_storage_profiles WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()
}

// --- Multipart uploads -------------------------------------------------------------------

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
struct ProfileRow {
    id: String,
    name: String,
    provider: String,
    endpoint: Option<String>,
    region: Option<String>,
    bucket: Option<String>,
    addressing: String,
    credential_source: String,
    access_key_id: Option<String>,
    account: Option<String>,
    secret_ref: Option<String>,
    session_token_ref: Option<String>,
    checksums: bool,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<ProfileRow> for ObjectStorageProfile {
    type Error = anyhow::Error;

    fn try_from(row: ProfileRow) -> Result<Self> {
        Ok(Self {
            id: row.id.parse()?,
            name: row.name,
            provider: parse_enum(&row.provider)?,
            endpoint: row.endpoint,
            region: row.region,
            bucket: row.bucket,
            addressing: parse_enum(&row.addressing)?,
            credential_source: parse_enum(&row.credential_source)?,
            access_key_id: row.access_key_id,
            account: row.account,
            has_secret: row.secret_ref.is_some(),
            has_session_token: row.session_token_ref.is_some(),
            secret_ref: row.secret_ref,
            session_token_ref: row.session_token_ref,
            checksums: row.checksums,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
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
