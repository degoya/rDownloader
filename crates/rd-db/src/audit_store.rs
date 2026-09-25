//! The audit log store (RD-110-03): the append, the bounded prune and the filtered read the
//! viewer and the export share.
//!
//! Nothing here redacts, for the reason `log_store` gives: the caller did it before the
//! record reached the writer, and a second pass here would hide a regression there behind a
//! safety net. Nothing here updates either, and that is not a convention — migration `0079`
//! carries a trigger that aborts any UPDATE on the table.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rd_core::{AuditAction, AuditActorKind, AuditOutcome};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, QueryBuilder, Sqlite, SqliteConnection, SqlitePool};

/// One record about to be stored. Every string reached this already redacted.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NewAuditRecord {
    pub recorded_at: DateTime<Utc>,
    pub action: AuditAction,
    pub outcome: AuditOutcome,
    pub actor_kind: AuditActorKind,
    /// An opaque handle: a token id, a session handle. Never a credential.
    pub actor_id: Option<String>,
    /// What a person called the actor, when there is such a name — a token's label.
    pub actor_label: Option<String>,
    pub client_address: Option<String>,
    /// The family of the thing acted on: `download`, `package`, `category`, `storage_root`,
    /// `plugin`, `token`, `settings`, `backup`.
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub target_name: Option<String>,
    pub trace_id: Option<String>,
    pub details: BTreeMap<String, String>,
}

impl NewAuditRecord {
    /// A record of `action` ending in `outcome`, with everything else left to the builders.
    #[must_use]
    pub fn new(action: AuditAction, outcome: AuditOutcome) -> Self {
        Self {
            recorded_at: Utc::now(),
            action,
            outcome,
            actor_kind: AuditActorKind::System,
            actor_id: None,
            actor_label: None,
            client_address: None,
            target_kind: None,
            target_id: None,
            target_name: None,
            trace_id: None,
            details: BTreeMap::new(),
        }
    }
}

/// One stored record.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AuditRecord {
    pub id: i64,
    pub recorded_at: DateTime<Utc>,
    pub action: AuditAction,
    pub outcome: AuditOutcome,
    pub actor_kind: AuditActorKind,
    pub actor_id: Option<String>,
    pub actor_label: Option<String>,
    pub client_address: Option<String>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub target_name: Option<String>,
    pub trace_id: Option<String>,
    pub details: BTreeMap<String, String>,
}

/// What a read asks for. Every filter is optional; `limit` is not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditQuery {
    pub action: Option<AuditAction>,
    pub outcome: Option<AuditOutcome>,
    pub actor_kind: Option<AuditActorKind>,
    pub actor_id: Option<String>,
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    pub trace_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// Records older than this id, for paging backwards.
    pub before_id: Option<i64>,
    pub limit: u32,
}

/// What one bounded prune did and what it left behind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditPruneReport {
    pub deleted: u64,
    /// Records still over the count cap after this batch; the caller runs again while it is
    /// non-zero, and yields to the queue in between.
    pub remaining_over_cap: u64,
}

#[derive(FromRow)]
struct Row {
    id: i64,
    recorded_at: String,
    action: String,
    outcome: String,
    actor_kind: String,
    actor_id: Option<String>,
    actor_label: Option<String>,
    client_address: Option<String>,
    target_kind: Option<String>,
    target_id: Option<String>,
    target_name: Option<String>,
    trace_id: Option<String>,
    details_json: Option<String>,
}

impl TryFrom<Row> for AuditRecord {
    type Error = anyhow::Error;

    fn try_from(row: Row) -> Result<Self> {
        let recorded_at = DateTime::parse_from_rfc3339(&row.recorded_at)
            .with_context(|| format!("audit record {} carries a bad timestamp", row.id))?
            .with_timezone(&Utc);
        let action = AuditAction::parse(&row.action)
            .with_context(|| format!("audit record {} names action {:?}", row.id, row.action))?;
        let outcome = AuditOutcome::parse(&row.outcome)
            .with_context(|| format!("audit record {} names outcome {:?}", row.id, row.outcome))?;
        let actor_kind = AuditActorKind::parse(&row.actor_kind)
            .with_context(|| format!("audit record {} names actor {:?}", row.id, row.actor_kind))?;
        let details = match row.details_json {
            Some(json) => serde_json::from_str(&json)
                .with_context(|| format!("audit record {} holds invalid details", row.id))?,
            None => BTreeMap::new(),
        };
        Ok(Self {
            id: row.id,
            recorded_at,
            action,
            outcome,
            actor_kind,
            actor_id: row.actor_id,
            actor_label: row.actor_label,
            client_address: row.client_address,
            target_kind: row.target_kind,
            target_id: row.target_id,
            target_name: row.target_name,
            trace_id: row.trace_id,
            details,
        })
    }
}

const COLUMNS: &str = "id, recorded_at, action, outcome, actor_kind, actor_id, actor_label, \
     client_address, target_kind, target_id, target_name, trace_id, details_json";

/// One timestamp shape for every row, so a lexical comparison in SQL is a chronological one.
fn timestamp(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Writes a batch in one transaction.
pub(crate) async fn append_audit_records(
    connection: &mut SqliteConnection,
    records: &[NewAuditRecord],
) -> Result<u64> {
    if records.is_empty() {
        return Ok(0);
    }
    let mut transaction = connection.begin().await?;
    for record in records {
        let details = if record.details.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&record.details)?)
        };
        sqlx::query(
            "INSERT INTO audit_records \
               (recorded_at, action, outcome, actor_kind, actor_id, actor_label, \
                client_address, target_kind, target_id, target_name, trace_id, details_json) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(timestamp(&record.recorded_at))
        .bind(record.action.as_str())
        .bind(record.outcome.as_str())
        .bind(record.actor_kind.as_str())
        .bind(&record.actor_id)
        .bind(&record.actor_label)
        .bind(&record.client_address)
        .bind(&record.target_kind)
        .bind(&record.target_id)
        .bind(&record.target_name)
        .bind(&record.trace_id)
        .bind(details)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(records.len() as u64)
}

/// The detail key that carries how many records a clear removed.
pub const CLEARED_DETAIL_KEY: &str = "removed_records";

/// Empties the audit log and writes `record` into it as the first new entry (RD-120-34).
///
/// One transaction, and that is the whole point. The delete and the entry that explains it
/// commit together or not at all, so there is no window in which the log is empty and
/// nothing says why — and no way for a second writer command to slip a record in between and
/// end up above the explanation. The count of removed rows is added to the record's details
/// here rather than by the caller, because the caller cannot know it before the delete ran.
pub(crate) async fn clear_audit_records(
    connection: &mut SqliteConnection,
    mut record: NewAuditRecord,
) -> Result<u64> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query("DELETE FROM audit_records")
        .execute(&mut *transaction)
        .await
        .context("clear audit records")?;
    let deleted = result.rows_affected();
    record
        .details
        .insert(CLEARED_DETAIL_KEY.to_owned(), deleted.to_string());
    let details = serde_json::to_string(&record.details)?;
    sqlx::query(
        "INSERT INTO audit_records \
           (recorded_at, action, outcome, actor_kind, actor_id, actor_label, \
            client_address, target_kind, target_id, target_name, trace_id, details_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(timestamp(&record.recorded_at))
    .bind(record.action.as_str())
    .bind(record.outcome.as_str())
    .bind(record.actor_kind.as_str())
    .bind(&record.actor_id)
    .bind(&record.actor_label)
    .bind(&record.client_address)
    .bind(&record.target_kind)
    .bind(&record.target_id)
    .bind(&record.target_name)
    .bind(&record.trace_id)
    .bind(Some(details))
    .execute(&mut *transaction)
    .await
    .context("record the audit clear in the emptied log")?;
    transaction.commit().await?;
    Ok(deleted)
}

/// Removes what retention no longer keeps, at most `batch` whole rows in this call.
///
/// Age first, then count, exactly as `log_store` does, and for the same reason: the writer
/// runs one command at a time, so a single unbounded `DELETE` over a neglected store would
/// stall every download state change for its duration. A row is removed whole — there is no
/// statement anywhere that changes a column of this table, and the trigger in migration
/// `0079` refuses one.
pub(crate) async fn prune_audit_records(
    connection: &mut SqliteConnection,
    max_records: u64,
    older_than: Option<DateTime<Utc>>,
    batch: u64,
) -> Result<AuditPruneReport> {
    let mut deleted = 0u64;
    if let Some(cutoff) = older_than
        && batch > 0
    {
        let result = sqlx::query(
            "DELETE FROM audit_records WHERE id IN \
               (SELECT id FROM audit_records WHERE recorded_at < ? ORDER BY id LIMIT ?)",
        )
        .bind(timestamp(&cutoff))
        .bind(i64::try_from(batch).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await
        .context("prune audit records by age")?;
        deleted += result.rows_affected();
    }
    let count = count_rows(&mut *connection).await?;
    let over_cap = count.saturating_sub(max_records);
    let room = batch.saturating_sub(deleted);
    let take = over_cap.min(room);
    if take > 0 {
        let result = sqlx::query(
            "DELETE FROM audit_records WHERE id IN \
               (SELECT id FROM audit_records ORDER BY id LIMIT ?)",
        )
        .bind(i64::try_from(take).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await
        .context("prune audit records by count")?;
        deleted += result.rows_affected();
    }
    Ok(AuditPruneReport {
        deleted,
        remaining_over_cap: over_cap.saturating_sub(take),
    })
}

async fn count_rows(connection: &mut SqliteConnection) -> Result<u64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_records")
        .fetch_one(connection)
        .await?;
    Ok(u64::try_from(count).unwrap_or(0))
}

pub(crate) async fn count_audit_records(pool: &SqlitePool) -> Result<u64> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_records")
        .fetch_one(pool)
        .await?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// The newest records matching the query, newest first.
pub(crate) async fn query_audit_records(
    pool: &SqlitePool,
    query: &AuditQuery,
) -> Result<Vec<AuditRecord>> {
    let mut builder: QueryBuilder<'_, Sqlite> =
        QueryBuilder::new(format!("SELECT {COLUMNS} FROM audit_records WHERE 1 = 1"));
    if let Some(action) = query.action {
        builder
            .push(" AND action = ")
            .push_bind(action.as_str().to_owned());
    }
    if let Some(outcome) = query.outcome {
        builder
            .push(" AND outcome = ")
            .push_bind(outcome.as_str().to_owned());
    }
    if let Some(kind) = query.actor_kind {
        builder
            .push(" AND actor_kind = ")
            .push_bind(kind.as_str().to_owned());
    }
    for (column, value) in [
        ("actor_id", query.actor_id.as_deref()),
        ("target_kind", query.target_kind.as_deref()),
        ("target_id", query.target_id.as_deref()),
        ("trace_id", query.trace_id.as_deref()),
    ] {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            builder
                .push(format!(" AND {column} = "))
                .push_bind(value.to_owned());
        }
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
    rows.into_iter().map(AuditRecord::try_from).collect()
}
