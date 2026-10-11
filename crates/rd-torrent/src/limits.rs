//! How many torrents download and seed at once (RD-1240-16).
//!
//! Downloads are bounded by the runner's slot count, which the queue asks on every dispatch
//! pass, so a waiting torrent stays queued until a place is free. Seeds have no queue: a
//! finished torrent seeds at once, and past the limit the seeding supervisor ends the seeds
//! that have seeded longest, so the newest finished torrents get their turn.

use std::sync::atomic::{AtomicUsize, Ordering};

use rd_core::DownloadId;

use crate::SharedTorrentSettings;

/// The active-download limit, read from the live settings on every dispatch pass.
///
/// `slot_capacity` is synchronous and the settings sit behind an async lock, so the read is a
/// `try_read`; while a writer holds the lock the value read last stands in.
pub(crate) struct ActiveDownloads {
    settings: SharedTorrentSettings,
    last: AtomicUsize,
}

impl ActiveDownloads {
    pub(crate) fn new(settings: SharedTorrentSettings) -> Self {
        let slots = Self {
            settings,
            last: AtomicUsize::new(slot_count(rd_core::DEFAULT_TORRENT_ACTIVE_DOWNLOADS)),
        };
        slots.get();
        slots
    }

    /// Torrents allowed to download right now.
    pub(crate) fn get(&self) -> usize {
        match self.settings.try_read() {
            Ok(settings) => {
                let slots = slot_count(settings.torrent_max_active_downloads);
                self.last.store(slots, Ordering::Release);
                slots
            }
            Err(_) => self.last.load(Ordering::Acquire),
        }
    }
}

/// The setting as a slot count, held to the range the settings endpoint accepts.
fn slot_count(configured: u32) -> usize {
    configured.clamp(1, rd_core::MAX_TORRENT_ACTIVE_DOWNLOADS) as usize
}

/// The seeds the supervisor ends because more than `limit` torrents seed.
///
/// `active` counts every seeding torrent, a seed whose data is still being hashed included;
/// `finished` holds the seeds a limit may end, with their seeded seconds. The longest seeded go
/// first (ties by id, so a pass is repeatable); a seed with incomplete data is never ended here.
pub(crate) fn seeds_over_limit(
    active: usize,
    mut finished: Vec<(DownloadId, u64)>,
    limit: Option<u32>,
) -> Vec<DownloadId> {
    let Some(limit) = limit else {
        return Vec::new();
    };
    let surplus = active.saturating_sub(limit as usize);
    finished.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    finished
        .into_iter()
        .take(surplus)
        .map(|(id, _)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rd_core::{DownloadId, TorrentSettings};
    use tokio::sync::RwLock;

    use super::{ActiveDownloads, seeds_over_limit};

    #[tokio::test]
    async fn the_download_limit_follows_the_live_settings() {
        let settings = Arc::new(RwLock::new(TorrentSettings::default()));
        let slots = ActiveDownloads::new(Arc::clone(&settings));
        assert_eq!(slots.get(), 4, "the default keeps the former fixed count");
        settings.write().await.torrent_max_active_downloads = 1;
        assert_eq!(slots.get(), 1);
        // While a writer holds the settings, the value read last stands in.
        let guard = settings.write().await;
        assert_eq!(slots.get(), 1);
        drop(guard);
        settings.write().await.torrent_max_active_downloads = 0;
        assert_eq!(slots.get(), 1, "never below one place");
        settings.write().await.torrent_max_active_downloads = 1_000;
        assert_eq!(slots.get(), 32, "never above the accepted range");
    }

    #[test]
    fn without_a_limit_no_seed_ends() {
        let finished = vec![(DownloadId::new(), 10), (DownloadId::new(), 20)];
        assert!(seeds_over_limit(2, finished, None).is_empty());
    }

    #[test]
    fn past_the_limit_the_longest_seeds_end_first() {
        let (old, middle, new) = (DownloadId::new(), DownloadId::new(), DownloadId::new());
        let finished = vec![(middle, 600), (new, 5), (old, 3_600)];
        assert!(seeds_over_limit(3, finished.clone(), Some(3)).is_empty());
        assert_eq!(seeds_over_limit(3, finished.clone(), Some(2)), [old]);
        assert_eq!(seeds_over_limit(3, finished, Some(1)), [old, middle]);
    }

    #[test]
    fn a_seed_still_hashing_counts_but_is_not_ended() {
        // Four seeds, one of them still checking its data: only the three finished ones can go.
        let (first, second, third) = (DownloadId::new(), DownloadId::new(), DownloadId::new());
        let finished = vec![(first, 30), (second, 20), (third, 10)];
        assert_eq!(
            seeds_over_limit(4, finished.clone(), Some(2)),
            [first, second]
        );
        assert_eq!(
            seeds_over_limit(4, finished, Some(0)),
            [first, second, third]
        );
    }
}
