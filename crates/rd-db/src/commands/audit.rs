//! The commands of `writer/audit.rs`.

use super::Reply;

/// The commands `Writer::handle_audit` applies.
// The variant names were the flat enum's; within one area they share a postfix.
#[allow(clippy::enum_variant_names)]
pub(crate) enum AuditCommand {
    /// Appends audit records (RD-110-03). There is deliberately no command that updates one.
    AppendAuditRecords {
        records: Vec<crate::NewAuditRecord>,
        reply: Reply<u64>,
    },
    /// Removes at most `batch` whole audit records that retention no longer keeps.
    PruneAuditRecords {
        max_records: u64,
        older_than: Option<chrono::DateTime<chrono::Utc>>,
        batch: u64,
        reply: Reply<crate::AuditPruneReport>,
    },
    /// Empties the audit log and writes `record` into it as the first new entry.
    ///
    /// The record travels with the command rather than being appended afterwards because the
    /// two must commit together: an audit log that is empty with nothing saying why has lost
    /// the one trace that explains it.
    ClearAuditRecords {
        record: Box<crate::NewAuditRecord>,
        reply: Reply<u64>,
    },
}
