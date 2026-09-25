//! Persistence of livestream channels watched by the recording monitor.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{CategoryId, EventEnvelope, EventKind, StreamChannel, StreamChannelId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, writer::insert_event};

/// Editable channel fields; `create` assigns the id and timestamps.
#[derive(Clone, Debug)]
pub struct NewStreamChannel {
    pub url: String,
    pub name: String,
    pub quality: Option<String>,
    pub category_id: Option<CategoryId>,
    pub enabled: bool,
    pub recording: rd_core::RecordingPolicy,
}

const COLUMNS: &str = "id, url, name, quality, category_id, enabled, last_live_at, \
     last_error, recording_json, created_at";

fn changed_event() -> EventEnvelope {
    EventEnvelope::new(
        EventKind::StreamChanged,
        serde_json::json!({ "resource": "stream_channel" }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<StreamChannel>> {
    sqlx::query_as::<_, ChannelRow>(&format!(
        "SELECT {COLUMNS} FROM stream_channels ORDER BY name, created_at"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewStreamChannel,
) -> Result<(StreamChannel, EventEnvelope)> {
    let value = StreamChannel {
        id: StreamChannelId::new(),
        url: input.url,
        name: input.name,
        quality: input.quality,
        category_id: input.category_id,
        enabled: input.enabled,
        last_live_at: None,
        last_error: None,
        recording: input.recording,
        created_at: Utc::now(),
    };
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO stream_channels (id, url, name, quality, category_id, enabled, \
         recording_json, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.url)
    .bind(&value.name)
    .bind(&value.quality)
    .bind(value.category_id.map(|id| id.to_string()))
    .bind(value.enabled)
    .bind(serde_json::to_string(&value.recording)?)
    .bind(value.created_at)
    .bind(value.created_at)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: StreamChannelId,
    input: NewStreamChannel,
) -> Result<(StreamChannel, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let updated = sqlx::query(
        "UPDATE stream_channels SET url = ?, name = ?, quality = ?, category_id = ?, \
         enabled = ?, recording_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&input.url)
    .bind(&input.name)
    .bind(&input.quality)
    .bind(input.category_id.map(|id| id.to_string()))
    .bind(input.enabled)
    .bind(serde_json::to_string(&input.recording)?)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("stream channel not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let value = sqlx::query_as::<_, ChannelRow>(&format!(
        "SELECT {COLUMNS} FROM stream_channels WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((value, event))
}

pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: StreamChannelId,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let deleted = sqlx::query("DELETE FROM stream_channels WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        anyhow::bail!(StoreError::not_found("stream channel not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Records the monitor's probe outcome: live timestamp and/or the latest error.
pub(crate) async fn touch(
    connection: &mut SqliteConnection,
    id: StreamChannelId,
    live_at: Option<DateTime<Utc>>,
    error: Option<String>,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query(
        "UPDATE stream_channels SET last_live_at = COALESCE(?, last_live_at), last_error = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(live_at)
    .bind(&error)
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

#[derive(FromRow)]
struct ChannelRow {
    id: String,
    url: String,
    name: String,
    quality: Option<String>,
    category_id: Option<String>,
    enabled: bool,
    last_live_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    recording_json: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<ChannelRow> for StreamChannel {
    type Error = anyhow::Error;

    fn try_from(row: ChannelRow) -> Result<Self> {
        Ok(Self {
            id: row.id.parse()?,
            url: row.url,
            name: row.name,
            quality: row.quality,
            category_id: row.category_id.as_deref().map(str::parse).transpose()?,
            enabled: row.enabled,
            last_live_at: row.last_live_at,
            last_error: row.last_error,
            // A blob written by a newer version falls back to the default rather than making
            // the whole channel unreadable.
            recording: row
                .recording_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok())
                .unwrap_or_default(),
            created_at: row.created_at,
        })
    }
}
