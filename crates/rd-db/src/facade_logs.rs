//! Database facade methods for the structured log store (RD-110-02).

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::{
    Database,
    commands::WriterCommand,
    log_store,
    log_store::{LogPruneReport, LogQuery, LogRecord, NewLogRecord},
    writer,
};

impl Database {
    /// Stores a batch of already-redacted records in one writer command.
    pub async fn append_log_records(&self, records: Vec<NewLogRecord>) -> Result<u64> {
        if records.is_empty() {
            return Ok(0);
        }
        writer::request(&self.writer, |reply| WriterCommand::AppendLogRecords {
            records,
            reply,
        })
        .await
    }

    /// Removes at most `batch` records that retention no longer keeps; see
    /// [`LogPruneReport::remaining_over_cap`] for when to call again.
    pub async fn prune_log_records(
        &self,
        max_records: u64,
        older_than: Option<DateTime<Utc>>,
        batch: u64,
    ) -> Result<LogPruneReport> {
        writer::request(&self.writer, |reply| WriterCommand::PruneLogRecords {
            max_records,
            older_than,
            batch,
            reply,
        })
        .await
    }

    /// Empties the log store on request and reports how many records went (RD-120-34).
    ///
    /// Deliberate and unbounded, unlike [`Database::prune_log_records`]; a person asked for
    /// it and is waiting for the answer.
    pub async fn clear_log_records(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::ClearLogRecords {
            reply,
        })
        .await
    }

    /// The newest records matching the query, newest first.
    pub async fn query_log_records(&self, query: &LogQuery) -> Result<Vec<LogRecord>> {
        log_store::query_log_records(&self.readers, query).await
    }

    /// How many records the store holds.
    pub async fn count_log_records(&self) -> Result<u64> {
        log_store::count_log_records(&self.readers).await
    }
}
