//! The commands of `writer/logs.rs`.

use super::Reply;

/// The commands `Writer::handle_logs` applies.
// The variant names were the flat enum's; within one area they share a postfix.
#[allow(clippy::enum_variant_names)]
pub(crate) enum LogsCommand {
    /// Stores a batch of already-redacted log records (RD-110-02).
    AppendLogRecords {
        records: Vec<crate::NewLogRecord>,
        reply: Reply<u64>,
    },
    /// Removes at most `batch` log records that retention no longer keeps.
    PruneLogRecords {
        max_records: u64,
        older_than: Option<chrono::DateTime<chrono::Utc>>,
        batch: u64,
        reply: Reply<crate::LogPruneReport>,
    },
    /// Empties the log store on request and reports how many records went (RD-120-34).
    ClearLogRecords { reply: Reply<u64> },
}
