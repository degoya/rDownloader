//! Storage roots: create, list, the default, edit and delete.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind, StorageRootConfig, StorageRootId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use super::{NewStorageRoot, STORAGE_ROOT_TAKEN, config_event};
use crate::{error::StoreError, parse_id, writer::insert_event};

pub(crate) async fn create_storage_root(
    connection: &mut SqliteConnection,
    id: StorageRootId,
    input: NewStorageRoot,
) -> Result<(StorageRootConfig, EventEnvelope)> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    // The first root is always the default. An install without one leaves destination
    // resolution with nothing to fall back to, and the flag is easy to miss in the form.
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM storage_roots")
        .fetch_one(&mut *tx)
        .await?;
    let value = StorageRootConfig {
        id,
        name: input.name,
        path: input.path,
        is_default: input.is_default || existing == 0,
        minimum_free_bytes: input.minimum_free_bytes,
    };
    let event = config_event(EventKind::CategoryChanged, "storage_root", value.id);
    // Clear the old default before inserting the new one: `idx_storage_roots_single_default`
    // rejects two default rows even mid-transaction, so the order is load-bearing.
    if value.is_default {
        sqlx::query("UPDATE storage_roots SET is_default = 0, updated_at = ?")
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO storage_roots (id, name, path, is_default, minimum_free_bytes, created_at, \
         updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(&value.path)
    .bind(value.is_default)
    .bind(value.minimum_free_bytes.map(persisted_bytes))
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|error| crate::error::tag_duplicate(error, STORAGE_ROOT_TAKEN))?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

/// The root marked default. Exactly one exists whenever the table is non-empty; the write
/// paths in this module maintain that, so callers need no alphabetical fallback.
pub(crate) async fn default_storage_root(pool: &SqlitePool) -> Result<Option<StorageRootConfig>> {
    sqlx::query_as::<_, StorageRootRow>(
        "SELECT id, name, path, is_default, minimum_free_bytes FROM storage_roots \
         WHERE is_default = 1 LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn list_storage_roots(pool: &SqlitePool) -> Result<Vec<StorageRootConfig>> {
    sqlx::query_as::<_, StorageRootRow>(
        "SELECT id, name, path, is_default, minimum_free_bytes FROM storage_roots \
         ORDER BY is_default DESC, name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn update_storage_root(
    connection: &mut SqliteConnection,
    id: StorageRootId,
    input: NewStorageRoot,
) -> Result<(StorageRootConfig, EventEnvelope)> {
    let now = Utc::now();
    let event = config_event(EventKind::CategoryChanged, "storage_root", id);
    let mut tx = connection.begin().await?;
    // Giving up the last default is not a thing a user can do: something has to stay the
    // fallback. Coerce rather than reject, and let the response tell the form what happened.
    let holds_only_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM storage_roots WHERE id = ? AND is_default = 1) \
         AND (SELECT COUNT(*) FROM storage_roots WHERE is_default = 1) <= 1",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let value = StorageRootConfig {
        id,
        name: input.name,
        path: input.path,
        is_default: input.is_default || holds_only_default,
        minimum_free_bytes: input.minimum_free_bytes,
    };
    if value.is_default {
        sqlx::query("UPDATE storage_roots SET is_default = 0, updated_at = ? WHERE id != ?")
            .bind(now)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    let updated = sqlx::query(
        "UPDATE storage_roots SET name = ?, path = ?, is_default = ?, minimum_free_bytes = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(&value.path)
    .bind(value.is_default)
    .bind(value.minimum_free_bytes.map(persisted_bytes))
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| crate::error::tag_duplicate(error, STORAGE_ROOT_TAKEN))?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("storage root not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn delete_storage_root(
    connection: &mut SqliteConnection,
    id: StorageRootId,
) -> Result<EventEnvelope> {
    let event = config_event(EventKind::CategoryChanged, "storage_root", id);
    let mut tx = connection.begin().await?;
    // `categories.storage_root_id` is NOT NULL, so a referenced root cannot be removed
    // without orphaning categories. Report it instead of letting the FK error surface.
    let categories: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE storage_root_id = ?")
            .bind(id.to_string())
            .fetch_one(&mut *tx)
            .await?;
    anyhow::ensure!(
        categories == 0,
        StoreError::in_use(format!(
            "storage root is still used by {categories} category(ies)"
        ))
    );
    let was_default: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM storage_roots WHERE id = ? AND is_default = 1)",
    )
    .bind(id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    let deleted = sqlx::query("DELETE FROM storage_roots WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("storage root not found"));
    }
    // Promote after the delete, never before: the partial unique index would otherwise see
    // two defaults. A no-op when that root was the last one.
    if was_default {
        sqlx::query(
            "UPDATE storage_roots SET is_default = 1, updated_at = ? \
             WHERE id = (SELECT id FROM storage_roots ORDER BY name LIMIT 1)",
        )
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// `ByteCount` is bounded by SQLite's INTEGER range when it is constructed, so the cast
/// cannot lose data; the saturating fallback only keeps the lint about panics happy.
fn persisted_bytes(value: rd_core::ByteCount) -> i64 {
    i64::try_from(value.get()).unwrap_or(i64::MAX)
}

#[derive(FromRow)]
struct StorageRootRow {
    id: String,
    name: String,
    path: String,
    is_default: bool,
    minimum_free_bytes: Option<i64>,
}
impl TryFrom<StorageRootRow> for StorageRootConfig {
    type Error = anyhow::Error;
    fn try_from(row: StorageRootRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            path: row.path,
            is_default: row.is_default,
            minimum_free_bytes: row
                .minimum_free_bytes
                .and_then(|value| u64::try_from(value).ok())
                .map(rd_core::ByteCount::new)
                .transpose()
                .map_err(|error| anyhow::anyhow!(error))?,
        })
    }
}
