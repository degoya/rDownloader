//! Persistence of object storage profiles (RD-150-04); the multipart uploads that survive a
//! restart are in `uploads`.
//!
//! Credential values never reach this module; it stores the opaque `vault://` references the
//! secret store minted, exactly like [`crate::remote_store`].

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{EventEnvelope, EventKind, ObjectStorageProfile, ObjectStorageProfileId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{enum_string, error::StoreError, parse_enum, writer::insert_event};

mod uploads;

pub use uploads::{ObjectUpload, ObjectUploadPart};
pub(crate) use uploads::{
    begin_upload, complete_upload, forget_uploads, record_part, upload, uploads,
};

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
    /// Whether an `ambient` profile may send the machine's credentials to its custom endpoint
    /// (RD-1190-20); `false` for every other profile.
    pub ambient_custom_endpoint: bool,
}

const COLUMNS: &str = "id, name, provider, endpoint, region, bucket, addressing, \
     credential_source, access_key_id, account, secret_ref, session_token_ref, checksums, \
     enabled, created_at, updated_at, ambient_custom_endpoint";

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
         checksums, enabled, created_at, updated_at, ambient_custom_endpoint) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(input.ambient_custom_endpoint)
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
         secret_ref = ?, session_token_ref = ?, checksums = ?, enabled = ?, updated_at = ?, \
         ambient_custom_endpoint = ? WHERE id = ?",
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
    .bind(input.ambient_custom_endpoint)
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
    ambient_custom_endpoint: bool,
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
            ambient_custom_endpoint: row.ambient_custom_endpoint,
        })
    }
}
