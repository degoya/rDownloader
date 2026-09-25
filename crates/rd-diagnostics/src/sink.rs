//! Drains the capture channel into the database and keeps the store within retention.
//!
//! Records are written in batches — one writer command per batch — so a chatty minute costs
//! the serialized writer a handful of transactions rather than a thousand. The sweep reads the
//! retention settings on every run, because a person who lowers them expects the store to
//! shrink without a restart, and deletes in bounded steps with a yield between them: the
//! writer serves queue mutations in the gaps, which is what "deletion never blocks the queue"
//! means in practice.

use std::time::Duration;

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use rd_core::{AuditRetentionSettings, LogRetentionSettings};
use rd_db::{Database, NewLogRecord};
use tokio::task::JoinHandle;

use crate::capture::LogStream;

/// Records per writer command.
pub const FLUSH_BATCH: usize = 200;
/// How long a record waits in the buffer at most.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
/// How often the sweep runs regardless of traffic.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// A sweep also runs after this many records since the last one.
pub const SWEEP_AFTER_RECORDS: u64 = 2_000;
/// Rows one prune command removes at most.
pub const PRUNE_BATCH: u64 = 2_000;
/// Audit rows one prune command removes at most. Smaller than the log batch on purpose: the
/// audit log is orders of magnitude quieter, so a sweep has little to do and there is no
/// reason for it to hold the writer for as long.
pub const AUDIT_PRUNE_BATCH: u64 = 500;

/// Starts the sink for the life of the process. It ends when every sender is gone.
pub fn spawn(stream: LogStream, database: Database) -> JoinHandle<()> {
    tokio::spawn(run(stream, database))
}

async fn run(mut stream: LogStream, database: Database) {
    let mut buffer: Vec<NewLogRecord> = Vec::with_capacity(FLUSH_BATCH);
    let mut since_sweep = 0u64;
    let mut flush_tick = tokio::time::interval(FLUSH_INTERVAL);
    let mut sweep_tick = tokio::time::interval(SWEEP_INTERVAL);
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sweep_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            received = stream.receiver.recv() => match received {
                Some(record) => {
                    buffer.push(record);
                    if buffer.len() >= FLUSH_BATCH {
                        since_sweep += flush(&database, &mut buffer).await;
                    }
                }
                None => {
                    flush(&database, &mut buffer).await;
                    return;
                }
            },
            _ = flush_tick.tick() => {
                since_sweep += flush(&database, &mut buffer).await;
            }
            _ = sweep_tick.tick() => {
                sweep(&database).await;
                since_sweep = 0;
            }
        }
        if since_sweep >= SWEEP_AFTER_RECORDS {
            sweep(&database).await;
            since_sweep = 0;
        }
    }
}

/// Writes the buffer and empties it; a failed write is reported once and the records are
/// dropped rather than retried, because a store that cannot be written must not grow a
/// backlog in memory that the next failure doubles.
async fn flush(database: &Database, buffer: &mut Vec<NewLogRecord>) -> u64 {
    if buffer.is_empty() {
        return 0;
    }
    let batch = std::mem::take(buffer);
    match database.append_log_records(batch).await {
        Ok(stored) => stored,
        Err(error) => {
            tracing::warn!(%error, "could not store log records");
            0
        }
    }
}

/// One retention sweep against the settings as they are now, for both stores.
///
/// The audit log is swept here rather than in a task of its own: it shares the settings read
/// and the writer, it needs the same "bounded batches with a yield between them" rule, and a
/// second timer would only add a second way for the writer to be busy.
async fn sweep(database: &Database) {
    let settings: LogRetentionSettings = match database.service_settings_or_default().await {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "could not read log retention settings");
            return;
        }
    };
    if let Err(error) = prune(database, &settings).await {
        tracing::warn!(%error, "log retention sweep failed");
    }
    let audit: AuditRetentionSettings = match database.service_settings_or_default().await {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!(%error, "could not read audit retention settings");
            return;
        }
    };
    if let Err(error) = prune_audit(database, &audit).await {
        tracing::warn!(%error, "audit retention sweep failed");
    }
}

/// Applies the audit retention in bounded steps until nothing is over the cap. Returns the
/// rows removed.
///
/// A record is removed *whole* — the table has no update path at all (migration `0079`
/// aborts one), so "retention" here can only mean deleting old rows and never editing them.
pub async fn prune_audit(database: &Database, settings: &AuditRetentionSettings) -> Result<u64> {
    let older_than = Utc::now() - ChronoDuration::days(i64::from(settings.audit_retention_days));
    let mut removed = 0u64;
    loop {
        let report = database
            .prune_audit_records(
                u64::from(settings.audit_retention_records),
                Some(older_than),
                AUDIT_PRUNE_BATCH,
            )
            .await?;
        removed += report.deleted;
        if report.remaining_over_cap == 0 {
            return Ok(removed);
        }
        // Let the writer serve whoever was waiting before the next batch.
        tokio::task::yield_now().await;
    }
}

/// Applies the retention in bounded steps until nothing is over the cap. Returns the rows
/// removed.
pub async fn prune(database: &Database, settings: &LogRetentionSettings) -> Result<u64> {
    let older_than = Utc::now() - ChronoDuration::days(i64::from(settings.log_retention_days));
    let mut removed = 0u64;
    loop {
        let report = database
            .prune_log_records(
                u64::from(settings.log_retention_records),
                Some(older_than),
                PRUNE_BATCH,
            )
            .await?;
        removed += report.deleted;
        if report.remaining_over_cap == 0 {
            return Ok(removed);
        }
        // Let the writer serve whoever was waiting before the next batch.
        tokio::task::yield_now().await;
    }
}

/// Stores whatever the channel holds right now. What the running sink does on a tick, for
/// callers that need the store current at a known point — tests, mostly.
pub async fn drain_now(stream: &mut LogStream, database: &Database) -> Result<u64> {
    let records = stream.drain_ready();
    if records.is_empty() {
        return Ok(0);
    }
    database.append_log_records(records).await
}
