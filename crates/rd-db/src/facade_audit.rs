//! Database facade methods for the audit log (RD-110-03).
//!
//! There is no `update_audit_record` and there never will be: migration `0079` carries a
//! trigger that aborts any UPDATE on the table, so a method added here would fail at runtime
//! rather than quietly work.

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::{
    Database,
    audit_store::{self, AuditPruneReport, AuditQuery, AuditRecord, NewAuditRecord},
    commands::WriterCommand,
    writer,
};

impl Database {
    /// Stores a batch of already-redacted audit records in one writer command.
    ///
    /// Awaited by the action that caused it, unlike a log record: the caller reports its own
    /// success only after this returned, which is what "the action produces an event
    /// atomically" means in a store that has one writer.
    pub async fn append_audit_records(&self, records: Vec<NewAuditRecord>) -> Result<u64> {
        if records.is_empty() {
            return Ok(0);
        }
        writer::request(&self.writer, |reply| WriterCommand::AppendAuditRecords {
            records,
            reply,
        })
        .await
    }

    /// Stores one record; the shape every caller in `rd-api` uses.
    pub async fn append_audit_record(&self, record: NewAuditRecord) -> Result<u64> {
        self.append_audit_records(vec![record]).await
    }

    /// Removes at most `batch` whole records that retention no longer keeps; see
    /// [`AuditPruneReport::remaining_over_cap`] for when to call again.
    pub async fn prune_audit_records(
        &self,
        max_records: u64,
        older_than: Option<DateTime<Utc>>,
        batch: u64,
    ) -> Result<AuditPruneReport> {
        writer::request(&self.writer, |reply| WriterCommand::PruneAuditRecords {
            max_records,
            older_than,
            batch,
            reply,
        })
        .await
    }

    /// Empties the audit log and writes `record` into it as its first new entry (RD-120-34).
    ///
    /// Returns how many records were removed; the same number is written into the record's
    /// details under [`audit_store::CLEARED_DETAIL_KEY`], because the caller cannot know it
    /// before the delete ran. Delete and entry commit together, so the log is never empty
    /// with nothing in it saying why.
    pub async fn clear_audit_records(&self, record: NewAuditRecord) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::ClearAuditRecords {
            record: Box::new(record),
            reply,
        })
        .await
    }

    /// The newest audit records matching the query, newest first.
    pub async fn query_audit_records(&self, query: &AuditQuery) -> Result<Vec<AuditRecord>> {
        audit_store::query_audit_records(&self.readers, query).await
    }

    /// How many audit records the store holds.
    pub async fn count_audit_records(&self) -> Result<u64> {
        audit_store::count_audit_records(&self.readers).await
    }
}
