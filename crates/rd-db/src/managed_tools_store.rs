//! Installed managed tool versions and the tool manifest's replay state (RD-102-02).
//!
//! The rows here are history, not truth about what is running: which versions this
//! installation downloaded and verified, and how far the signed manifest has advanced. Which
//! version is *active* lives in the `active.json` pointer beside the version directories, so
//! that an activation is one rename on the same filesystem as the files it activates.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// One installed tool version.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct ManagedToolRecord {
    pub name: String,
    pub version: String,
    pub installed_at: String,
    /// Where the bytes came from.
    pub source_url: String,
    /// The hex SHA-256 that was verified before promotion.
    pub sha256: String,
}

/// A version about to be recorded.
#[derive(Clone, Debug)]
pub struct NewManagedTool {
    pub name: String,
    pub version: String,
    pub source_url: String,
    pub sha256: String,
}

/// How far the signed tool manifest has advanced for this installation.
#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct ToolManifestState {
    /// The highest sequence accepted so far; anything at or below it is a replay.
    pub sequence: i64,
    pub issued_at: String,
    pub accepted_at: String,
}

pub(crate) async fn list_managed_tools(pool: &SqlitePool) -> Result<Vec<ManagedToolRecord>> {
    let rows = sqlx::query_as::<_, ManagedToolRecord>(
        "SELECT name, version, installed_at, source_url, sha256 \
         FROM managed_tools ORDER BY name, installed_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub(crate) async fn tool_manifest_state(pool: &SqlitePool) -> Result<Option<ToolManifestState>> {
    let row = sqlx::query_as::<_, ToolManifestState>(
        "SELECT sequence, issued_at, accepted_at FROM tool_manifest_state WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Records an installed version, replacing an earlier record of the same name and version.
pub(crate) async fn record_managed_tool(
    connection: &mut SqliteConnection,
    input: NewManagedTool,
) -> Result<(ManagedToolRecord, EventEnvelope)> {
    let value = ManagedToolRecord {
        name: input.name,
        version: input.version,
        installed_at: Utc::now().to_rfc3339(),
        source_url: input.source_url,
        sha256: input.sha256,
    };
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO managed_tools (name, version, installed_at, source_url, sha256) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(name, version) DO UPDATE SET \
           installed_at = excluded.installed_at, \
           source_url = excluded.source_url, \
           sha256 = excluded.sha256",
    )
    .bind(&value.name)
    .bind(&value.version)
    .bind(&value.installed_at)
    .bind(&value.source_url)
    .bind(&value.sha256)
    .execute(&mut *transaction)
    .await?;
    let event = tool_event(&value.name, &value.version);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((value, event))
}

/// Forgets one installed version; returns whether a row was removed.
pub(crate) async fn forget_managed_tool(
    connection: &mut SqliteConnection,
    name: &str,
    version: &str,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query("DELETE FROM managed_tools WHERE name = ? AND version = ?")
        .bind(name)
        .bind(version)
        .execute(&mut *transaction)
        .await?;
    let event = tool_event(name, version);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((result.rows_affected() > 0, event))
}

/// Raises the accepted manifest sequence.
///
/// `MAX(sequence, excluded.sequence)` rather than a plain assignment: two concurrent refreshes
/// must not let the older of them lower the replay floor that the newer one already raised.
pub(crate) async fn accept_tool_manifest(
    connection: &mut SqliteConnection,
    sequence: i64,
    issued_at: &str,
) -> Result<EventEnvelope> {
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO tool_manifest_state (id, sequence, issued_at, accepted_at) \
         VALUES (1, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           sequence = MAX(tool_manifest_state.sequence, excluded.sequence), \
           issued_at = excluded.issued_at, \
           accepted_at = excluded.accepted_at",
    )
    .bind(sequence)
    .bind(issued_at)
    .bind(Utc::now().to_rfc3339())
    .execute(&mut *transaction)
    .await?;
    let event = EventEnvelope::new(
        EventKind::ManagedToolChanged,
        serde_json::json!({ "resource": "tool_manifest", "sequence": sequence }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// What an install or a forget announces.
///
/// Name and version, never `source_url` or `sha256`: the first is a mirror URL that can carry
/// a token in a deployment that uses one, and the second belongs to the verification record,
/// not to a broadcast. Both are already reachable through `GET /api/v1/system/tools` for a
/// caller that may read them.
fn tool_event(name: &str, version: &str) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::ManagedToolChanged,
        serde_json::json!({ "resource": "tool", "name": name, "version": version }),
    )
}
