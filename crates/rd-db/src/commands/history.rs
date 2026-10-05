//! The commands of `writer/history.rs`.

use super::Reply;

/// The commands `Writer::handle_history` applies (RD-1100-04).
pub(crate) enum HistoryCommand {
    /// Removes the entries the retention no longer keeps.
    PruneHistory {
        max_entries: u64,
        older_than: chrono::DateTime<chrono::Utc>,
        reply: Reply<u64>,
    },
    /// Empties the history on request and reports how many entries went.
    ClearHistory { reply: Reply<u64> },
    /// Hides entries from the SABnzbd history; `None` hides every one.
    HideHistoryFromCompat {
        package_ids: Option<Vec<rd_core::PackageId>>,
        reply: Reply<u64>,
    },
}
