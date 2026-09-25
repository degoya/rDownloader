//! Live statistics of one torrent.
//!
//! Three shapes with three transports, because they scale differently. Aggregates are a
//! handful of numbers and are cheap enough to broadcast; the peer list is unbounded and is
//! paginated over REST; the piece bitfield grows with the torrent and is folded into a
//! fixed number of buckets before it leaves the service.
//!
//! Peer addresses are personal data of third parties: they are never logged, never written
//! into a persisted event, and masked to their network prefix unless the operator opts in.

use anyhow::Result;
use rd_core::{
    ByteCount, DownloadId, TorrentAggregateStats, TorrentPeerEntry, TorrentPeerPage,
    TorrentPieceAvailability, mask_peer_address,
};

use crate::TorrentService;

/// Byte count that never fails, so one oversized counter cannot break a whole snapshot.
fn bytes(value: u64) -> ByteCount {
    ByteCount::new(value).unwrap_or_default()
}

impl TorrentService {
    /// Aggregate counters of one torrent.
    ///
    /// A row that is not in the session answers with `live: false` and the last persisted
    /// numbers rather than presenting stale values as current.
    pub async fn aggregate_stats(&self, id: DownloadId) -> Result<TorrentAggregateStats> {
        let generation = self.session_generation().await;
        let state = self.job_state(id).await;
        let seeded_seconds = state.seed.seeded_seconds(chrono::Utc::now());
        let offline = |total: u64| TorrentAggregateStats {
            live: false,
            progress_bytes: bytes(0),
            total_bytes: bytes(total),
            uploaded_bytes: bytes(0),
            download_bps: 0,
            upload_bps: 0,
            peer_count: 0,
            piece_count: state
                .metadata
                .as_ref()
                .map_or(0, |metadata| metadata.piece_count),
            pieces_have: 0,
            ratio: 0.0,
            seeded_seconds,
            sampled_at: chrono::Utc::now(),
            session_generation: generation,
        };
        let total = state
            .metadata
            .as_ref()
            .map_or(0, |metadata| metadata.total_bytes.get());

        let Some(entry) = self.inner.registry.read().await.get(id).cloned() else {
            return Ok(offline(total));
        };
        let session = self.session().await?;
        let Some(handle) = session.get(entry.handle()) else {
            return Ok(offline(total));
        };
        let stats = handle.stats();
        let live = stats.live.as_ref();
        Ok(TorrentAggregateStats {
            live: live.is_some(),
            progress_bytes: bytes(stats.progress_bytes),
            total_bytes: bytes(stats.total_bytes),
            uploaded_bytes: bytes(stats.uploaded_bytes),
            download_bps: live.map_or(0, |live| live.download_speed.mbps as u64 * 125_000),
            upload_bps: live.map_or(0, |live| live.upload_speed.mbps as u64 * 125_000),
            peer_count: live.map_or(0, |live| live.snapshot.peer_stats.live),
            piece_count: state
                .metadata
                .as_ref()
                .map_or(0, |metadata| metadata.piece_count),
            pieces_have: live.map_or(0, |live| {
                u32::try_from(live.snapshot.downloaded_and_checked_pieces).unwrap_or(u32::MAX)
            }),
            ratio: TorrentAggregateStats::compute_ratio(stats.uploaded_bytes, stats.total_bytes),
            seeded_seconds,
            sampled_at: chrono::Utc::now(),
            session_generation: generation,
        })
    }

    /// One page of the peer list.
    ///
    /// Sorted by address so paging is stable between calls, and cursor-based so a large
    /// swarm never produces an unbounded payload.
    pub async fn peer_page(
        &self,
        id: DownloadId,
        limit: usize,
        cursor: Option<String>,
    ) -> Result<TorrentPeerPage> {
        let reveal = self
            .inner
            .settings
            .read()
            .await
            .torrent_peer_addresses_visible;
        let sampled_at = chrono::Utc::now();
        let empty = TorrentPeerPage {
            peers: Vec::new(),
            total: 0,
            next_cursor: None,
            live: false,
            sampled_at,
        };
        let Some(entry) = self.inner.registry.read().await.get(id).cloned() else {
            return Ok(empty);
        };
        let session = self.session().await?;
        let api = librqbit::api::Api::new(session, None);
        let Ok(snapshot) = api.api_peer_stats(entry.handle(), Default::default()) else {
            // The torrent is not live; an empty page is the honest answer.
            return Ok(empty);
        };
        // `PeerStats` has no public re-export path, so the type stays inferred.
        let mut peers: Vec<_> = snapshot.peers.into_iter().collect();
        peers.sort_by(|left, right| left.0.cmp(&right.0));
        let total = u32::try_from(peers.len()).unwrap_or(u32::MAX);
        let start = cursor
            .and_then(|cursor| peers.iter().position(|(address, _)| *address > cursor))
            .unwrap_or(0);
        let limit = limit.clamp(1, rd_core::MAX_PEER_PAGE);
        let page: Vec<_> = peers.into_iter().skip(start).take(limit + 1).collect();
        let has_more = page.len() > limit;
        let page = &page[..page.len().min(limit)];
        Ok(TorrentPeerPage {
            next_cursor: has_more
                .then(|| page.last().map(|(address, _)| address.clone()))
                .flatten(),
            peers: page
                .iter()
                .map(|(address, stats)| TorrentPeerEntry {
                    address: if reveal {
                        address.clone()
                    } else {
                        mask_peer_address(address)
                    },
                    client: stats.client_name.clone(),
                    state: stats.state.to_owned(),
                    connection: stats
                        .conn_kind
                        .map(|kind| format!("{kind:?}").to_lowercase()),
                    downloaded_bytes: bytes(stats.counters.fetched_bytes),
                    uploaded_bytes: bytes(stats.counters.uploaded_bytes),
                })
                .collect(),
            total,
            live: true,
            sampled_at,
        })
    }

    /// Bucketed piece availability of one torrent.
    pub async fn piece_availability(&self, id: DownloadId) -> Result<TorrentPieceAvailability> {
        let sampled_at = chrono::Utc::now();
        let Some(entry) = self.inner.registry.read().await.get(id).cloned() else {
            return Ok(TorrentPieceAvailability::from_bitfield(
                &[],
                sampled_at,
                false,
            ));
        };
        let session = self.session().await?;
        let api = librqbit::api::Api::new(session, None);
        let Ok((have, total)) = api.api_dump_haves(entry.handle()) else {
            return Ok(TorrentPieceAvailability::from_bitfield(
                &[],
                sampled_at,
                false,
            ));
        };
        let pieces: Vec<bool> = (0..total as usize)
            .map(|index| have.get(index).is_some_and(|bit| *bit))
            .collect();
        Ok(TorrentPieceAvailability::from_bitfield(
            &pieces, sampled_at, true,
        ))
    }

    /// Broadcasts the aggregate counters of one torrent.
    ///
    /// Deliberately transient: the event is published but never persisted, so a long
    /// download does not fill the event table with samples nobody reads afterwards. Peer
    /// addresses are not part of the payload.
    pub(crate) async fn broadcast_stats(&self, id: DownloadId) {
        let Ok(stats) = self.aggregate_stats(id).await else {
            return;
        };
        let Ok(payload) = serde_json::to_value(stats) else {
            return;
        };
        self.inner.database.broadcast(rd_core::EventEnvelope::new(
            rd_core::EventKind::TorrentStats,
            serde_json::json!({ "download_id": id, "stats": payload }),
        ));
    }

    /// The aggregate counters of every registered torrent, for the broadcast loop.
    pub(crate) async fn registered_ids(&self) -> Vec<DownloadId> {
        self.inner
            .registry
            .read()
            .await
            .snapshot()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }
}

/// Interval between two statistics broadcasts.
///
/// Slower than the progress tick on purpose: these numbers are read by a detail panel a
/// human is looking at, and one sample every two seconds is already more than the eye
/// resolves.
pub const STATS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Background loop broadcasting aggregate counters for every active torrent.
pub(crate) async fn broadcast_loop(service: TorrentService) {
    loop {
        tokio::select! {
            () = service.inner.shutdown.cancelled() => return,
            () = tokio::time::sleep(STATS_INTERVAL) => {}
        }
        for id in service.registered_ids().await {
            service.broadcast_stats(id).await;
        }
    }
}

/// Parses the peer page size from a query parameter.
#[must_use]
pub fn peer_page_size(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(rd_core::DEFAULT_PEER_PAGE)
        .clamp(1, rd_core::MAX_PEER_PAGE)
}

#[cfg(test)]
mod tests {
    use super::peer_page_size;

    #[test]
    fn the_peer_page_size_is_bounded_in_both_directions() {
        assert_eq!(peer_page_size(None), rd_core::DEFAULT_PEER_PAGE);
        assert_eq!(peer_page_size(Some(0)), 1);
        assert_eq!(peer_page_size(Some(10)), 10);
        // A client cannot ask for an unbounded page.
        assert_eq!(peer_page_size(Some(100_000)), rd_core::MAX_PEER_PAGE);
    }
}
