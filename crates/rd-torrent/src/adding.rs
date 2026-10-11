//! Adding a torrent to the session under an id of its own (RD-1240-28).
//!
//! librqbit picks a new torrent's id as the highest *persisted* id plus one, read before the new
//! entry is stored, and answers "already managed" for a torrent whose id is taken. Two torrents
//! added at once -- every queued torrent when the session starts lazily -- got the same id: the
//! second add came back with the first one's handle, failed the stored metadata
//! (`torrent.metadata_mismatch`, permanent) and deleted that handle, which left the first torrent
//! in librqbit's broken `None` state. The id is handed out here instead, above every torrent the
//! session holds and every id handed out before.

use anyhow::Result;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Session};

use crate::TorrentService;

impl TorrentService {
    /// Adds `request` to `session` under a fresh id.
    pub(crate) async fn add_to(
        &self,
        session: &std::sync::Arc<Session>,
        request: AddTorrent<'_>,
        mut options: AddTorrentOptions,
    ) -> Result<AddTorrentResponse> {
        options.preferred_id = Some(self.next_torrent_id(session));
        session.add_torrent(request, Some(options)).await
    }

    /// An id no torrent of `session` holds and none handed out before.
    fn next_torrent_id(&self, session: &Session) -> usize {
        let highest = session.with_torrents(|torrents| torrents.map(|(id, _)| id).max());
        let floor = highest.map_or(0, |id| id.saturating_add(1));
        let mut next = self
            .inner
            .next_torrent_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = (*next).max(floor);
        *next = id.saturating_add(1);
        id
    }
}

/// Whether an add answered with another torrent than the one asked for: `expected` is the
/// info hash the row was reviewed with, when it has one.
pub(crate) fn is_foreign(response_hash: &str, expected: Option<&str>) -> bool {
    expected.is_some_and(|expected| !expected.eq_ignore_ascii_case(response_hash))
}

#[cfg(test)]
#[path = "adding_tests.rs"]
mod tests;
