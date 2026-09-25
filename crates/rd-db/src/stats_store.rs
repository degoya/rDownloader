//! Persistent transfer statistics (RD-110-01): hourly buckets that age into daily ones, and
//! the all-time totals the metrics counters read.
//!
//! See `migrations/0077_transfer_stats.sql` for the two tables and why there are two. The
//! writes happen inside the transaction that completes or fails a download, so a figure here
//! never disagrees with the queue it describes; the sweep that thins the buckets works in
//! bounded batches so the writer is never held for longer than one batch takes.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// Most rows one [`prune`] call touches. The writer applies commands strictly in order, so
/// this is the longest a queue write can wait behind the sweep.
pub const PRUNE_BATCH: u32 = 500;

/// The provider recorded for a transfer that used no account.
pub const DIRECT_PROVIDER: &str = "direct";

/// The bucket width a row is kept at.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsResolution {
    Hour,
    Day,
}

impl StatsResolution {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    /// The start of the bucket `moment` falls into, as the table stores it.
    #[must_use]
    pub fn bucket_start(self, moment: DateTime<Utc>) -> String {
        match self {
            Self::Hour => moment.format("%Y-%m-%dT%H:00:00Z").to_string(),
            Self::Day => moment.format("%Y-%m-%dT00:00:00Z").to_string(),
        }
    }
}

/// How long the buckets are kept, in days.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatsRetention {
    /// Hourly rows older than this are added into their day's row.
    pub hourly_days: u32,
    /// Rows older than this are deleted.
    pub retention_days: u32,
}

/// What one [`prune`] call did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatsPruneReport {
    /// Hourly rows folded into a daily one.
    pub downsampled: u64,
    /// Rows deleted for being past the retention.
    pub deleted: u64,
    /// Whether the batch was full, so another call has work left.
    pub more: bool,
}

/// One bucket of one kind and provider.
#[derive(Clone, Debug, Deserialize, Eq, FromRow, PartialEq, Serialize)]
pub struct TransferBucket {
    pub bucket_start: String,
    pub kind: String,
    pub provider: String,
    pub completed: i64,
    pub failed: i64,
    pub retries: i64,
    pub bytes: i64,
    pub seconds: i64,
}

/// The all-time figures of one kind and provider.
#[derive(Clone, Debug, Deserialize, Eq, FromRow, PartialEq, Serialize)]
pub struct TransferTotal {
    pub kind: String,
    pub provider: String,
    pub completed: i64,
    pub failed: i64,
    pub retries: i64,
    pub bytes: i64,
    pub seconds: i64,
}

/// What a transfer did, as the statistics count it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TransferOutcome {
    /// Finished; `seconds` is the time from creation to completion.
    Completed { bytes: u64, seconds: u64 },
    /// Failed and scheduled for another attempt.
    Retried,
    /// Failed for good, or blocked until a person acts.
    Failed,
}

/// The provider id behind an account, or [`DIRECT_PROVIDER`] without one.
pub(crate) async fn provider_of(
    connection: &mut SqliteConnection,
    account_id: Option<&str>,
) -> Result<String> {
    let Some(account_id) = account_id else {
        return Ok(DIRECT_PROVIDER.to_owned());
    };
    let provider: Option<String> = sqlx::query_scalar("SELECT provider FROM accounts WHERE id = ?")
        .bind(account_id)
        .fetch_optional(connection)
        .await?;
    Ok(provider.unwrap_or_else(|| DIRECT_PROVIDER.to_owned()))
}

/// Adds one outcome to the current hour's bucket and to the totals.
pub(crate) async fn record(
    connection: &mut SqliteConnection,
    kind: &str,
    provider: &str,
    outcome: TransferOutcome,
    now: DateTime<Utc>,
) -> Result<()> {
    let (completed, failed, retries, bytes, seconds) = match outcome {
        TransferOutcome::Completed { bytes, seconds } => (1, 0, 0, bytes, seconds),
        TransferOutcome::Retried => (0, 0, 1, 0, 0),
        TransferOutcome::Failed => (0, 1, 0, 0, 0),
    };
    let bytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    sqlx::query(
        "INSERT INTO transfer_stats \
           (resolution, bucket_start, kind, provider, completed, failed, retries, bytes, seconds) \
         VALUES ('hour', ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(resolution, bucket_start, kind, provider) DO UPDATE SET \
           completed = completed + excluded.completed, \
           failed = failed + excluded.failed, \
           retries = retries + excluded.retries, \
           bytes = bytes + excluded.bytes, \
           seconds = seconds + excluded.seconds",
    )
    .bind(StatsResolution::Hour.bucket_start(now))
    .bind(kind)
    .bind(provider)
    .bind(completed)
    .bind(failed)
    .bind(retries)
    .bind(bytes)
    .bind(seconds)
    .execute(&mut *connection)
    .await
    .context("record transfer bucket")?;
    sqlx::query(
        "INSERT INTO transfer_totals \
           (kind, provider, completed, failed, retries, bytes, seconds) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(kind, provider) DO UPDATE SET \
           completed = completed + excluded.completed, \
           failed = failed + excluded.failed, \
           retries = retries + excluded.retries, \
           bytes = bytes + excluded.bytes, \
           seconds = seconds + excluded.seconds",
    )
    .bind(kind)
    .bind(provider)
    .bind(completed)
    .bind(failed)
    .bind(retries)
    .bind(bytes)
    .bind(seconds)
    .execute(connection)
    .await
    .context("record transfer total")?;
    Ok(())
}

/// Every bucket at `resolution` that starts at or after `since`, oldest first.
pub(crate) async fn list_buckets(
    pool: &SqlitePool,
    resolution: StatsResolution,
    since: DateTime<Utc>,
) -> Result<Vec<TransferBucket>> {
    sqlx::query_as::<_, TransferBucket>(
        "SELECT bucket_start, kind, provider, completed, failed, retries, bytes, seconds \
         FROM transfer_stats WHERE resolution = ? AND bucket_start >= ? \
         ORDER BY bucket_start, kind, provider",
    )
    .bind(resolution.as_str())
    .bind(resolution.bucket_start(since))
    .fetch_all(pool)
    .await
    .context("list transfer buckets")
}

/// The all-time totals, by kind and provider.
pub(crate) async fn list_totals(pool: &SqlitePool) -> Result<Vec<TransferTotal>> {
    sqlx::query_as::<_, TransferTotal>(
        "SELECT kind, provider, completed, failed, retries, bytes, seconds \
         FROM transfer_totals ORDER BY kind, provider",
    )
    .fetch_all(pool)
    .await
    .context("list transfer totals")
}

/// How many rows the statistics hold, buckets and all-time totals together (RD-120-34).
///
/// Both tables, because both are what a person means by "the statistics": leaving
/// `transfer_totals` behind would empty every chart and still show an all-time figure nobody
/// could account for.
pub(crate) async fn count_rows(pool: &SqlitePool) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM transfer_stats) + (SELECT COUNT(*) FROM transfer_totals)",
    )
    .fetch_one(pool)
    .await
    .context("count transfer statistics")?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// Empties both statistics tables in one transaction and reports how many rows went.
///
/// Unbounded where [`prune`] is bounded, for the reason `log_store::clear_log_records` gives:
/// a person waited for this after a confirmation, and a half-cleared statistic is the mixed
/// state they asked to be rid of.
pub(crate) async fn clear(connection: &mut SqliteConnection) -> Result<u64> {
    let mut transaction = sqlx::Connection::begin(connection).await?;
    let buckets = sqlx::query("DELETE FROM transfer_stats")
        .execute(&mut *transaction)
        .await
        .context("clear transfer buckets")?
        .rows_affected();
    let totals = sqlx::query("DELETE FROM transfer_totals")
        .execute(&mut *transaction)
        .await
        .context("clear transfer totals")?
        .rows_affected();
    transaction.commit().await?;
    Ok(buckets + totals)
}

/// One bounded pass of the retention sweep.
///
/// Folds at most [`PRUNE_BATCH`] hourly rows older than `hourly_days` into their day's row,
/// then deletes rows older than `retention_days` with whatever is left of the batch. The
/// caller repeats while `more` is set; between two calls every other writer command gets its
/// turn, which is the whole reason the batch is bounded.
pub(crate) async fn prune(
    connection: &mut SqliteConnection,
    retention: StatsRetention,
    now: DateTime<Utc>,
) -> Result<StatsPruneReport> {
    let hourly_cutoff = StatsResolution::Hour
        .bucket_start(now - chrono::Duration::days(retention.hourly_days.into()));
    let retention_cutoff = StatsResolution::Day
        .bucket_start(now - chrono::Duration::days(retention.retention_days.into()));
    let mut transaction = sqlx::Connection::begin(connection).await?;
    // The same subquery names the rows twice, and it names the same rows both times: the
    // insert only adds `day` rows, which the `hour` filter never sees.
    const STALE_HOURS: &str = "SELECT rowid FROM transfer_stats \
         WHERE resolution = 'hour' AND bucket_start < ? ORDER BY bucket_start LIMIT ?";
    sqlx::query(&format!(
        "INSERT INTO transfer_stats \
           (resolution, bucket_start, kind, provider, completed, failed, retries, bytes, seconds) \
         SELECT 'day', substr(bucket_start, 1, 10) || 'T00:00:00Z', kind, provider, \
                SUM(completed), SUM(failed), SUM(retries), SUM(bytes), SUM(seconds) \
         FROM transfer_stats WHERE rowid IN ({STALE_HOURS}) \
         GROUP BY substr(bucket_start, 1, 10), kind, provider \
         ON CONFLICT(resolution, bucket_start, kind, provider) DO UPDATE SET \
           completed = completed + excluded.completed, \
           failed = failed + excluded.failed, \
           retries = retries + excluded.retries, \
           bytes = bytes + excluded.bytes, \
           seconds = seconds + excluded.seconds"
    ))
    .bind(&hourly_cutoff)
    .bind(PRUNE_BATCH)
    .execute(&mut *transaction)
    .await
    .context("downsample transfer buckets")?;
    let downsampled = sqlx::query(&format!(
        "DELETE FROM transfer_stats WHERE rowid IN ({STALE_HOURS})"
    ))
    .bind(&hourly_cutoff)
    .bind(PRUNE_BATCH)
    .execute(&mut *transaction)
    .await
    .context("remove downsampled buckets")?
    .rows_affected();
    let budget = u64::from(PRUNE_BATCH).saturating_sub(downsampled);
    let deleted = if budget == 0 {
        0
    } else {
        sqlx::query(
            "DELETE FROM transfer_stats WHERE rowid IN \
             (SELECT rowid FROM transfer_stats WHERE bucket_start < ? LIMIT ?)",
        )
        .bind(&retention_cutoff)
        .bind(i64::try_from(budget).unwrap_or(i64::MAX))
        .execute(&mut *transaction)
        .await
        .context("delete expired buckets")?
        .rows_affected()
    };
    transaction.commit().await?;
    Ok(StatsPruneReport {
        downsampled,
        deleted,
        more: downsampled + deleted >= u64::from(PRUNE_BATCH),
    })
}
