//! The structured log store (RD-110-02): batched inserts, bounded pruning and the filtered
//! read the viewer and the diagnostic bundle share.
//!
//! Nothing here redacts. The layer in `rd-diagnostics` did that before a record reached the
//! writer, and doing it twice would hide a regression there behind a safety net here; the
//! contract is that this table never sees an unredacted byte, and the tests in
//! `crates/rd-diagnostics` hold that line where it is drawn.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rd_core::LogLevel;
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, QueryBuilder, Sqlite, SqliteConnection, SqlitePool};

/// One record about to be stored.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NewLogRecord {
    pub recorded_at: DateTime<Utc>,
    pub level: LogLevel,
    /// The `tracing` target, such as `rd_http::engine`.
    pub component: String,
    /// The event's stable `code` field, when it carried one.
    pub code: Option<String>,
    pub correlation_id: Option<String>,
    pub message: String,
    /// Every other field of the event, rendered as text.
    pub fields: BTreeMap<String, String>,
}

/// One stored record.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LogRecord {
    pub id: i64,
    pub recorded_at: DateTime<Utc>,
    pub level: LogLevel,
    pub component: String,
    pub code: Option<String>,
    pub correlation_id: Option<String>,
    pub message: String,
    pub fields: BTreeMap<String, String>,
}

/// What a read asks for. Every filter is optional; `limit` is not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogQuery {
    /// This level and the more severe ones.
    pub min_level: Option<LogLevel>,
    /// A component prefix: `rd_http` matches `rd_http::engine`.
    pub component: Option<String>,
    pub code: Option<String>,
    pub correlation_id: Option<String>,
    /// A case-insensitive substring of the message.
    pub search: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Records older than this id, for paging backwards.
    pub before_id: Option<i64>,
    pub limit: u32,
}

/// What one bounded prune did and what it left behind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogPruneReport {
    pub deleted: u64,
    /// Records still over the count cap after this batch; the caller runs again while it is
    /// non-zero, and yields to the queue in between.
    pub remaining_over_cap: u64,
}

#[derive(FromRow)]
struct Row {
    id: i64,
    recorded_at: String,
    level: String,
    component: String,
    code: Option<String>,
    correlation_id: Option<String>,
    message: String,
    fields_json: Option<String>,
}

impl TryFrom<Row> for LogRecord {
    type Error = anyhow::Error;

    fn try_from(row: Row) -> Result<Self> {
        let recorded_at = DateTime::parse_from_rfc3339(&row.recorded_at)
            .with_context(|| format!("log record {} carries a bad timestamp", row.id))?
            .with_timezone(&Utc);
        let level = LogLevel::parse(&row.level)
            .with_context(|| format!("log record {} carries level {:?}", row.id, row.level))?;
        let fields = match row.fields_json {
            Some(json) => serde_json::from_str(&json)
                .with_context(|| format!("log record {} holds invalid fields", row.id))?,
            None => BTreeMap::new(),
        };
        Ok(Self {
            id: row.id,
            recorded_at,
            level,
            component: row.component,
            code: row.code,
            correlation_id: row.correlation_id,
            message: row.message,
            fields,
        })
    }
}

const COLUMNS: &str =
    "id, recorded_at, level, component, code, correlation_id, message, fields_json";

/// One timestamp shape for every row, so a lexical comparison in SQL is a chronological one.
fn timestamp(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Writes a batch in one transaction.
pub(crate) async fn append_log_records(
    connection: &mut SqliteConnection,
    records: &[NewLogRecord],
) -> Result<u64> {
    if records.is_empty() {
        return Ok(0);
    }
    let mut transaction = connection.begin().await?;
    for record in records {
        let fields = if record.fields.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&record.fields)?)
        };
        sqlx::query(
            "INSERT INTO log_records \
               (recorded_at, level, component, code, correlation_id, message, fields_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(timestamp(&record.recorded_at))
        .bind(record.level.as_str())
        .bind(&record.component)
        .bind(&record.code)
        .bind(&record.correlation_id)
        .bind(&record.message)
        .bind(fields)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(records.len() as u64)
}

/// Empties the store in one statement and reports how many records went (RD-120-34).
///
/// Unbounded on purpose, where [`prune_log_records`] is bounded: the prune runs unattended
/// against a store that may be neglected, so it yields to the queue between batches. This one
/// is a deliberate act a person waited for after a confirmation, and a clear that stopped
/// halfway would leave exactly the mixed state the person asked to be rid of.
pub(crate) async fn clear_log_records(connection: &mut SqliteConnection) -> Result<u64> {
    let result = sqlx::query("DELETE FROM log_records")
        .execute(&mut *connection)
        .await
        .context("clear log records")?;
    Ok(result.rows_affected())
}

/// Removes what retention no longer keeps, at most `batch` rows in this call.
///
/// Age first, then count: a record older than `older_than` goes whatever the count, and the
/// oldest records go while more than `max_records` remain. The batch cap is what keeps a
/// queue mutation from waiting behind a sweep of a neglected store — the writer runs one
/// command at a time, so a single unbounded `DELETE` of a million rows would stall every
/// download state change for its duration.
pub(crate) async fn prune_log_records(
    connection: &mut SqliteConnection,
    max_records: u64,
    older_than: Option<DateTime<Utc>>,
    batch: u64,
) -> Result<LogPruneReport> {
    let mut deleted = 0u64;
    if let Some(cutoff) = older_than
        && batch > 0
    {
        let result = sqlx::query(
            "DELETE FROM log_records WHERE id IN \
               (SELECT id FROM log_records WHERE recorded_at < ? ORDER BY id LIMIT ?)",
        )
        .bind(timestamp(&cutoff))
        .bind(i64::try_from(batch).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await
        .context("prune log records by age")?;
        deleted += result.rows_affected();
    }
    let count = count_rows(&mut *connection).await?;
    let over_cap = count.saturating_sub(max_records);
    let room = batch.saturating_sub(deleted);
    let take = over_cap.min(room);
    if take > 0 {
        let result = sqlx::query(
            "DELETE FROM log_records WHERE id IN \
               (SELECT id FROM log_records ORDER BY id LIMIT ?)",
        )
        .bind(i64::try_from(take).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await
        .context("prune log records by count")?;
        deleted += result.rows_affected();
    }
    Ok(LogPruneReport {
        deleted,
        remaining_over_cap: over_cap.saturating_sub(take),
    })
}

async fn count_rows(connection: &mut SqliteConnection) -> Result<u64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_records")
        .fetch_one(connection)
        .await?;
    Ok(u64::try_from(count).unwrap_or(0))
}

pub(crate) async fn count_log_records(pool: &SqlitePool) -> Result<u64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM log_records")
        .fetch_one(pool)
        .await?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// `LIKE` treats `%` and `_` as wildcards; a person searching for `100%` means the characters.
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// The newest records matching the query, newest first.
pub(crate) async fn query_log_records(
    pool: &SqlitePool,
    query: &LogQuery,
) -> Result<Vec<LogRecord>> {
    let mut builder: QueryBuilder<'_, Sqlite> =
        QueryBuilder::new(format!("SELECT {COLUMNS} FROM log_records WHERE 1 = 1"));
    if let Some(min_level) = query.min_level {
        builder.push(" AND level IN (");
        let mut separated = builder.separated(", ");
        for level in min_level.and_above() {
            separated.push_bind(level.as_str());
        }
        separated.push_unseparated(")");
    }
    if let Some(component) = query.component.as_deref().filter(|value| !value.is_empty()) {
        builder
            .push(" AND component LIKE ")
            .push_bind(format!("{}%", escape_like(component)))
            .push(" ESCAPE '\\'");
    }
    if let Some(code) = query.code.as_deref().filter(|value| !value.is_empty()) {
        builder.push(" AND code = ").push_bind(code.to_owned());
    }
    if let Some(correlation) = query
        .correlation_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        builder
            .push(" AND correlation_id = ")
            .push_bind(correlation.to_owned());
    }
    if let Some(search) = query.search.as_deref().filter(|value| !value.is_empty()) {
        builder
            .push(" AND message LIKE ")
            .push_bind(format!("%{}%", escape_like(search)))
            .push(" ESCAPE '\\'");
    }
    if let Some(since) = query.since {
        builder
            .push(" AND recorded_at >= ")
            .push_bind(timestamp(&since));
    }
    if let Some(until) = query.until {
        builder
            .push(" AND recorded_at <= ")
            .push_bind(timestamp(&until));
    }
    if let Some(before) = query.before_id {
        builder.push(" AND id < ").push_bind(before);
    }
    builder
        .push(" ORDER BY id DESC LIMIT ")
        .push_bind(i64::from(query.limit));
    let rows = builder.build_query_as::<Row>().fetch_all(pool).await?;
    rows.into_iter().map(LogRecord::try_from).collect()
}
