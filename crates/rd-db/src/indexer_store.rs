//! Persistence of the Newznab indexers a person defines once (RD-180-19).
//!
//! Credential values never reach this module; it stores the opaque `vault://` reference the
//! secret store minted, exactly like [`crate::object_storage_store`].

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{EventEnvelope, EventKind, Indexer, IndexerId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};
use url::Url;

use crate::{error::StoreError, writer::insert_event};

/// Editable indexer fields, for a new indexer and for an update alike.
#[derive(Clone, Debug)]
pub struct NewIndexer {
    pub name: String,
    pub url: Url,
    /// `None` on an update keeps the stored key, so a form that does not resend it does not
    /// drop it.
    pub secret_ref: Option<String>,
    pub categories: Vec<String>,
    pub enabled: bool,
}

const COLUMNS: &str = "id, name, url, secret_ref, categories_json, enabled, created_at, updated_at";

const DUPLICATE: &str = "an indexer with this name already exists";

fn changed_event() -> EventEnvelope {
    // An indexer belongs to the Usenet settings page, which reloads its lists on this event.
    EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({ "resource": "indexer" }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Indexer>> {
    sqlx::query_as::<_, IndexerRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM indexers ORDER BY name"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn get(pool: &SqlitePool, id: IndexerId) -> Result<Option<Indexer>> {
    sqlx::query_as::<_, IndexerRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM indexers WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewIndexer,
) -> Result<(Indexer, EventEnvelope)> {
    let now = Utc::now();
    let id = IndexerId::new();
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO indexers (id, name, url, secret_ref, categories_json, enabled, created_at, \
         updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&input.name)
    .bind(input.url.as_str())
    .bind(input.secret_ref.as_deref())
    .bind(serde_json::to_string(&input.categories)?)
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

/// Applies an update and returns the indexer plus the key reference it replaced, if any.
pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: IndexerId,
    input: NewIndexer,
) -> Result<(Indexer, Option<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let previous: Option<Option<String>> =
        sqlx::query_scalar("SELECT secret_ref FROM indexers WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    let previous = previous.context(StoreError::not_found("indexer not found"))?;
    // The replaced reference goes back to the caller for the vault; an unchanged key survives.
    let orphan = match (&input.secret_ref, &previous) {
        (Some(new), Some(old)) if new != old => Some(old.clone()),
        _ => None,
    };
    let secret_ref = input.secret_ref.or(previous);
    sqlx::query(
        "UPDATE indexers SET name = ?, url = ?, secret_ref = ?, categories_json = ?, \
         enabled = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&input.name)
    .bind(input.url.as_str())
    .bind(secret_ref.as_deref())
    .bind(serde_json::to_string(&input.categories)?)
    .bind(input.enabled)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await
    .map_err(|error| crate::error::tag_duplicate(error, DUPLICATE))?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let value = fetch(connection, id).await?;
    Ok((value, orphan, event))
}

/// Deletes an indexer and returns its key reference so the caller can drop the vault entry.
///
/// Nothing else points at an indexer: a subscription that took one over holds its own copy of
/// the address and the key (RD-180-20), so deleting it breaks no subscription.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: IndexerId,
) -> Result<(Option<String>, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let secret_ref: Option<Option<String>> =
        sqlx::query_scalar("SELECT secret_ref FROM indexers WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    let secret_ref = secret_ref.context(StoreError::not_found("indexer not found"))?;
    sqlx::query("DELETE FROM indexers WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((secret_ref, event))
}

async fn fetch(connection: &mut SqliteConnection, id: IndexerId) -> Result<Indexer> {
    sqlx::query_as::<_, IndexerRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM indexers WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()
}

#[derive(FromRow)]
struct IndexerRow {
    id: String,
    name: String,
    url: String,
    secret_ref: Option<String>,
    categories_json: String,
    enabled: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<IndexerRow> for Indexer {
    type Error = anyhow::Error;

    fn try_from(row: IndexerRow) -> Result<Self> {
        Ok(Self {
            id: row.id.parse()?,
            name: row.name,
            url: Url::parse(&row.url)?,
            has_secret: row.secret_ref.is_some(),
            secret_ref: row.secret_ref,
            categories: serde_json::from_str(&row.categories_json)
                .context("indexer categories are unreadable")?,
            enabled: row.enabled,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}
