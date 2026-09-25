//! Sanitised history of what installed plugins did and how it ended.
//!
//! Bounded on purpose: the newest [`MAX_EXECUTIONS_PER_PLUGIN`] entries of each plugin are
//! kept and the rest are dropped on insert. Diagnostics that grow without limit stop being
//! diagnostics and become a disk-space bug, and nothing here is worth a backup.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// Entries kept per plugin. Enough to see a pattern, small enough to never matter.
pub const MAX_EXECUTIONS_PER_PLUGIN: i64 = 50;

/// One recorded plugin invocation.
#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct PluginExecution {
    pub id: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub plugin_type: String,
    /// Which entry point ran: `resolve`, `check`, `probe`, `run`, …
    pub operation: String,
    /// Bare UUID a user can quote; it identifies the entry and nothing else.
    pub correlation_id: String,
    /// `ok`, `crash`, `timeout`, `fuel`, `memory`, `host_error` or `denied`.
    pub outcome: String,
    pub error_class: Option<String>,
    /// Redacted before it was written; never a URL, a credential or a server reply.
    pub message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: i64,
}

/// A new entry, before the store gives it an id.
#[derive(Clone, Debug)]
pub struct NewPluginExecution {
    pub plugin_id: String,
    pub plugin_version: String,
    pub plugin_type: String,
    pub operation: String,
    pub correlation_id: String,
    pub outcome: String,
    pub error_class: Option<String>,
    pub message: Option<String>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: i64,
}

pub(crate) async fn list_plugin_executions(
    pool: &SqlitePool,
    plugin_id: &str,
    limit: i64,
) -> Result<Vec<PluginExecution>> {
    Ok(sqlx::query_as::<_, PluginExecution>(
        "SELECT id, plugin_id, plugin_version, plugin_type, operation, correlation_id, outcome, \
         error_class, message, started_at, duration_ms FROM plugin_executions \
         WHERE plugin_id = ? ORDER BY started_at DESC, rowid DESC LIMIT ?",
    )
    .bind(plugin_id)
    .bind(limit.clamp(1, MAX_EXECUTIONS_PER_PLUGIN))
    .fetch_all(pool)
    .await?)
}

/// How many entries each plugin has, for the plugins that have any.
///
/// The plugin manager needs to know *whether* a plugin recorded anything before it offers the
/// accordion that shows the entries; it must not have to read the entries to find out. One
/// grouped read over an index answers it for every plugin at once, and the table it scans is
/// bounded by construction — [`MAX_EXECUTIONS_PER_PLUGIN`] rows per plugin, trimmed on every
/// insert — so this cannot become an unbounded read either.
///
/// Plugins with no entry are absent from the result rather than present with a zero; the
/// caller already knows which ids it asked about.
pub(crate) async fn plugin_execution_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as::<_, (String, i64)>(
        "SELECT plugin_id, COUNT(*) FROM plugin_executions GROUP BY plugin_id",
    )
    .fetch_all(pool)
    .await?)
}

pub(crate) async fn record_plugin_execution(
    connection: &mut SqliteConnection,
    entry: NewPluginExecution,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO plugin_executions (id, plugin_id, plugin_version, plugin_type, operation, \
         correlation_id, outcome, error_class, message, started_at, duration_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::now_v7().to_string())
    .bind(&entry.plugin_id)
    .bind(&entry.plugin_version)
    .bind(&entry.plugin_type)
    .bind(&entry.operation)
    .bind(&entry.correlation_id)
    .bind(&entry.outcome)
    .bind(&entry.error_class)
    .bind(&entry.message)
    .bind(entry.started_at)
    .bind(entry.duration_ms)
    .execute(&mut *connection)
    .await?;
    // Trim in the same write, so the table cannot grow between two inserts.
    sqlx::query(
        "DELETE FROM plugin_executions WHERE plugin_id = ? AND id NOT IN ( \
           SELECT id FROM plugin_executions WHERE plugin_id = ? \
           ORDER BY started_at DESC, rowid DESC LIMIT ? )",
    )
    .bind(&entry.plugin_id)
    .bind(&entry.plugin_id)
    .bind(MAX_EXECUTIONS_PER_PLUGIN)
    .execute(&mut *connection)
    .await?;
    Ok(())
}
