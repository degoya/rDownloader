//! Which queue row maps to which torrent in the session.
//!
//! Before this existed only finished torrents were tracked, which meant the API had no way
//! to reach a torrent that was still downloading. Every control surface — file plan
//! changes, tracker edits, reannounce, statistics — needs the handle of a running torrent,
//! so the registry covers both phases and is the single place that knows the mapping.

use std::collections::HashMap;

use rd_core::DownloadId;

/// What a registered torrent is currently doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TorrentPhase {
    Downloading,
    Seeding,
}

/// One registered torrent.
#[derive(Clone, Debug)]
pub(crate) struct TorrentEntry {
    pub torrent_id: usize,
    pub info_hash: String,
    pub phase: TorrentPhase,
    /// When the last manual reannounce ran, for rate limiting.
    pub last_reannounce: Option<std::time::Instant>,
    /// Session incarnation this entry was added to. An entry from an older incarnation is
    /// stale and must be re-added before it can be used.
    pub generation: u64,
}

impl TorrentEntry {
    /// The engine-side handle of this torrent.
    #[must_use]
    pub fn handle(&self) -> librqbit::api::TorrentIdOrHash {
        librqbit::api::TorrentIdOrHash::Id(self.torrent_id)
    }
}

/// The `DownloadId` to torrent mapping.
#[derive(Default)]
pub(crate) struct Registry {
    entries: HashMap<DownloadId, TorrentEntry>,
}

impl Registry {
    /// Adds or replaces the entry for one queue row.
    pub fn register(
        &mut self,
        download_id: DownloadId,
        torrent_id: usize,
        info_hash: String,
        phase: TorrentPhase,
        generation: u64,
    ) {
        self.entries.insert(
            download_id,
            TorrentEntry {
                torrent_id,
                info_hash,
                phase,
                last_reannounce: None,
                generation,
            },
        );
    }

    /// Moves an existing entry into another phase; no-op when it is not registered.
    pub fn set_phase(&mut self, download_id: DownloadId, phase: TorrentPhase) {
        if let Some(entry) = self.entries.get_mut(&download_id) {
            entry.phase = phase;
        }
    }

    /// Records that a reannounce just ran.
    pub fn mark_reannounce(&mut self, download_id: DownloadId, at: std::time::Instant) {
        if let Some(entry) = self.entries.get_mut(&download_id) {
            entry.last_reannounce = Some(at);
        }
    }

    /// Drops the entry for one queue row, returning it.
    pub fn forget(&mut self, download_id: DownloadId) -> Option<TorrentEntry> {
        self.entries.remove(&download_id)
    }

    /// The entry of one queue row.
    #[must_use]
    pub fn get(&self, download_id: DownloadId) -> Option<&TorrentEntry> {
        self.entries.get(&download_id)
    }

    /// Every registered row, cloned so the lock is not held while the engine is queried.
    #[must_use]
    pub fn snapshot(&self) -> Vec<(DownloadId, TorrentEntry)> {
        self.entries
            .iter()
            .map(|(id, entry)| (*id, entry.clone()))
            .collect()
    }

    /// Every row in the given phase.
    #[must_use]
    pub fn in_phase(&self, phase: TorrentPhase) -> Vec<(DownloadId, TorrentEntry)> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.phase == phase)
            .map(|(id, entry)| (*id, entry.clone()))
            .collect()
    }

    /// Drops every entry from an older session incarnation, returning the dropped rows.
    ///
    /// Called after a session rebuild: those torrent ids belong to a session that no longer
    /// exists, so keeping them would hand out handles the engine cannot resolve.
    pub fn retire_before(&mut self, generation: u64) -> Vec<(DownloadId, TorrentEntry)> {
        let stale: Vec<DownloadId> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.generation < generation)
            .map(|(id, _)| *id)
            .collect();
        stale
            .into_iter()
            .filter_map(|id| self.entries.remove(&id).map(|entry| (id, entry)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use rd_core::DownloadId;

    use super::{Registry, TorrentPhase};

    #[test]
    fn a_row_can_be_registered_read_and_forgotten() {
        let id = DownloadId::new();
        let mut registry = Registry::default();
        registry.register(id, 7, "hash".to_owned(), TorrentPhase::Downloading, 1);
        let entry = registry.get(id).expect("registered");
        assert_eq!(entry.torrent_id, 7);
        assert_eq!(entry.phase, TorrentPhase::Downloading);
        assert!(registry.forget(id).is_some());
        assert!(registry.get(id).is_none());
    }

    #[test]
    fn the_phase_moves_from_downloading_to_seeding() {
        let id = DownloadId::new();
        let mut registry = Registry::default();
        registry.register(id, 1, "hash".to_owned(), TorrentPhase::Downloading, 1);
        registry.set_phase(id, TorrentPhase::Seeding);
        assert_eq!(registry.in_phase(TorrentPhase::Seeding).len(), 1);
        assert!(registry.in_phase(TorrentPhase::Downloading).is_empty());
    }

    #[test]
    fn a_rebuild_retires_entries_of_the_previous_session() {
        let old = DownloadId::new();
        let fresh = DownloadId::new();
        let mut registry = Registry::default();
        registry.register(old, 1, "a".to_owned(), TorrentPhase::Seeding, 1);
        registry.register(fresh, 2, "b".to_owned(), TorrentPhase::Seeding, 2);
        let retired = registry.retire_before(2);
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].0, old);
        assert!(registry.get(fresh).is_some());
    }

    #[test]
    fn setting_a_phase_on_an_unknown_row_is_harmless() {
        let mut registry = Registry::default();
        registry.set_phase(DownloadId::new(), TorrentPhase::Seeding);
        assert!(registry.snapshot().is_empty());
    }
}
