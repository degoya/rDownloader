//! Database facade methods for the download history (RD-1100-04).

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::{
    Database,
    commands::HistoryCommand,
    history_store,
    history_store::{HistoryPage, HistoryQuery},
    writer,
};

impl Database {
    /// The entries matching the query, newest first, and how many match in all.
    pub async fn list_download_history(&self, query: &HistoryQuery) -> Result<HistoryPage> {
        history_store::list(&self.readers, query).await
    }

    /// Every entry the SABnzbd adapter reports, newest first: all but the ones a client
    /// deleted from its history.
    pub async fn list_compat_history(&self) -> Result<Vec<rd_core::HistoryEntry>> {
        let query = HistoryQuery {
            compat_visible_only: true,
            ..HistoryQuery::default()
        };
        Ok(history_store::list(&self.readers, &query).await?.entries)
    }

    /// One entry by its id.
    pub async fn get_history_entry(&self, id: i64) -> Result<Option<rd_core::HistoryEntry>> {
        history_store::get(&self.readers, id).await
    }

    /// Removes the entries that ended before `older_than`, then the oldest beyond
    /// `max_entries`; returns how many went.
    pub async fn prune_download_history(
        &self,
        max_entries: u64,
        older_than: DateTime<Utc>,
    ) -> Result<u64> {
        writer::request(&self.writer, |reply| HistoryCommand::PruneHistory {
            max_entries,
            older_than,
            reply,
        })
        .await
    }

    /// Empties the history on request and reports how many entries went.
    pub async fn clear_download_history(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| HistoryCommand::ClearHistory { reply }).await
    }

    /// Hides entries from the history the SABnzbd adapter reports, `None` every one; the
    /// native history keeps them. What a SABnzbd client's "delete from history" does.
    pub async fn hide_history_from_compat(
        &self,
        package_ids: Option<Vec<rd_core::PackageId>>,
    ) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            HistoryCommand::HideHistoryFromCompat { package_ids, reply }
        })
        .await
    }
}
