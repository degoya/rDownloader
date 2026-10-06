//! Bounded retention for the append-only record tables, audit and log (RD-1120-12).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;

use crate::timestamp;

/// What one bounded prune did and what it left behind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub deleted: u64,
    /// Records still over the count cap after this batch; the caller runs again while it is
    /// non-zero, and yields to the queue in between.
    pub remaining_over_cap: u64,
}

/// A table retention prunes. Both have an integer `id` in insertion order and a
/// `recorded_at` stored by [`timestamp`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum RecordTable {
    Audit,
    Log,
}

impl RecordTable {
    const fn name(self) -> &'static str {
        match self {
            Self::Audit => "audit_records",
            Self::Log => "log_records",
        }
    }

    const fn noun(self) -> &'static str {
        match self {
            Self::Audit => "audit records",
            Self::Log => "log records",
        }
    }
}

/// Removes what retention no longer keeps, at most `batch` whole rows in this call.
///
/// Age first, then count: a record older than `older_than` goes whatever the count, and the
/// oldest records go while more than `max_records` remain. The batch cap is what keeps a
/// queue mutation from waiting behind a sweep of a neglected store — the writer runs one
/// command at a time, so a single unbounded `DELETE` of a million rows would stall every
/// download state change for its duration. A row is removed whole: no statement anywhere
/// changes a column of the audit table, and the trigger in migration `0079` refuses one.
pub(crate) async fn prune_records(
    connection: &mut SqliteConnection,
    table: RecordTable,
    max_records: u64,
    older_than: Option<DateTime<Utc>>,
    batch: u64,
) -> Result<PruneReport> {
    let name = table.name();
    let mut deleted = 0u64;
    if let Some(cutoff) = older_than
        && batch > 0
    {
        // The table name is one of `RecordTable`'s constants, never input.
        let result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {name} WHERE id IN \
               (SELECT id FROM {name} WHERE recorded_at < ? ORDER BY id LIMIT ?)"
        )))
        .bind(timestamp(&cutoff))
        .bind(i64::try_from(batch).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await
        .with_context(|| format!("prune {} by age", table.noun()))?;
        deleted += result.rows_affected();
    }
    let count = count_rows(&mut *connection, table).await?;
    let over_cap = count.saturating_sub(max_records);
    let room = batch.saturating_sub(deleted);
    let take = over_cap.min(room);
    if take > 0 {
        let result = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DELETE FROM {name} WHERE id IN \
               (SELECT id FROM {name} ORDER BY id LIMIT ?)"
        )))
        .bind(i64::try_from(take).unwrap_or(i64::MAX))
        .execute(&mut *connection)
        .await
        .with_context(|| format!("prune {} by count", table.noun()))?;
        deleted += result.rows_affected();
    }
    Ok(PruneReport {
        deleted,
        remaining_over_cap: over_cap.saturating_sub(take),
    })
}

async fn count_rows(connection: &mut SqliteConnection, table: RecordTable) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM {}",
        table.name()
    )))
    .fetch_one(connection)
    .await?;
    Ok(u64::try_from(count).unwrap_or(0))
}
