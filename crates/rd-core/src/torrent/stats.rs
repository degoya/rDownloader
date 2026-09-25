//! Live torrent statistics as they cross the API boundary.
//!
//! Three shapes with three different transports: aggregates are cheap and go out over SSE,
//! peers are unbounded and are paginated over REST, and the piece bitfield is bucketed to a
//! fixed width so its payload does not grow with the torrent.
//!
//! Peer addresses are personal data of third parties. They are never written to a log and
//! never put into a persisted event; the API masks them unless the operator opts in.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ByteCount;

/// Number of buckets the piece bitfield is folded into, regardless of torrent size.
pub const PIECE_BUCKETS: usize = 512;

/// Default number of peers returned by one page.
pub const DEFAULT_PEER_PAGE: usize = 50;

/// Largest peer page a client may request.
pub const MAX_PEER_PAGE: usize = 200;

/// Aggregate counters of one torrent. Cheap enough to broadcast.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, ToSchema)]
pub struct TorrentAggregateStats {
    /// Whether these numbers come from a torrent currently in the session. When `false`,
    /// the values are the last known ones and must not be presented as current.
    pub live: bool,
    pub progress_bytes: ByteCount,
    pub total_bytes: ByteCount,
    pub uploaded_bytes: ByteCount,
    pub download_bps: u64,
    pub upload_bps: u64,
    pub peer_count: u32,
    pub piece_count: u32,
    pub pieces_have: u32,
    /// Uploaded divided by total payload size; `0` while nothing has been uploaded.
    pub ratio: f64,
    /// Total seconds spent seeding, across restarts.
    pub seeded_seconds: u64,
    pub sampled_at: chrono::DateTime<chrono::Utc>,
    /// Session incarnation the sample came from. A changed generation tells a client that
    /// the engine was rebuilt and older samples are not comparable.
    pub session_generation: u64,
}

impl TorrentAggregateStats {
    /// Ratio derived from the two counters, guarding against a zero payload size.
    #[must_use]
    pub fn compute_ratio(uploaded_bytes: u64, total_bytes: u64) -> f64 {
        if total_bytes == 0 {
            return 0.0;
        }
        uploaded_bytes as f64 / total_bytes as f64
    }
}

/// One connected peer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentPeerEntry {
    /// Peer address, masked to the network prefix unless the operator opted into full
    /// addresses. Never logged and never persisted.
    pub address: String,
    /// Client name advertised in the handshake, when known.
    pub client: Option<String>,
    /// Engine-reported connection state (`live`, `connecting`, `queued`, …).
    pub state: String,
    /// Transport of the connection (`tcp`, `utp`), when known.
    pub connection: Option<String>,
    pub downloaded_bytes: ByteCount,
    pub uploaded_bytes: ByteCount,
}

/// One page of peers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentPeerPage {
    pub peers: Vec<TorrentPeerEntry>,
    /// Total number of peers known for this torrent, not just the page.
    pub total: u32,
    /// Opaque cursor for the next page; `None` on the last page.
    pub next_cursor: Option<String>,
    pub live: bool,
    pub sampled_at: chrono::DateTime<chrono::Utc>,
}

/// Bucketed piece availability, so the payload stays constant for a 50-piece torrent and a
/// 50 000-piece one alike.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentPieceAvailability {
    pub piece_count: u32,
    /// How many pieces one bucket covers.
    pub pieces_per_bucket: u32,
    /// Percentage of complete pieces per bucket, 0-100. Never longer than
    /// [`PIECE_BUCKETS`], so the payload does not grow with the torrent.
    pub buckets: Vec<u8>,
    pub live: bool,
    pub sampled_at: chrono::DateTime<chrono::Utc>,
}

impl TorrentPieceAvailability {
    /// Folds a have-bitfield into [`PIECE_BUCKETS`] percentage buckets.
    #[must_use]
    pub fn from_bitfield(
        have: &[bool],
        sampled_at: chrono::DateTime<chrono::Utc>,
        live: bool,
    ) -> Self {
        let piece_count = have.len();
        if piece_count == 0 {
            return Self {
                piece_count: 0,
                pieces_per_bucket: 0,
                buckets: Vec::new(),
                live,
                sampled_at,
            };
        }
        let bucket_count = PIECE_BUCKETS.min(piece_count);
        let pieces_per_bucket = piece_count.div_ceil(bucket_count);
        let buckets = have
            .chunks(pieces_per_bucket)
            .map(|chunk| {
                let complete = chunk.iter().filter(|piece| **piece).count();
                u8::try_from(complete * 100 / chunk.len()).unwrap_or(100)
            })
            .collect();
        Self {
            piece_count: u32::try_from(piece_count).unwrap_or(u32::MAX),
            pieces_per_bucket: u32::try_from(pieces_per_bucket).unwrap_or(u32::MAX),
            buckets,
            live,
            sampled_at,
        }
    }
}

/// Masks a peer address down to its network prefix.
///
/// IPv4 keeps the first three octets, IPv6 keeps the routing prefix. The port is dropped
/// entirely: together with a full address it identifies one connection of one person.
#[must_use]
pub fn mask_peer_address(address: &str) -> String {
    let host = address
        .rsplit_once(':')
        .map_or(address, |(host, _)| host)
        .trim_matches(['[', ']']);
    if let Ok(parsed) = host.parse::<std::net::Ipv4Addr>() {
        let [a, b, c, _] = parsed.octets();
        return format!("{a}.{b}.{c}.x");
    }
    if let Ok(parsed) = host.parse::<std::net::Ipv6Addr>() {
        let segments = parsed.segments();
        return format!(
            "{:x}:{:x}:{:x}:{:x}::x",
            segments[0], segments[1], segments[2], segments[3]
        );
    }
    super::metadata::TRACKER_REDACTION_PLACEHOLDER.to_owned()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        PIECE_BUCKETS, TorrentAggregateStats, TorrentPieceAvailability, mask_peer_address,
    };

    #[test]
    fn ratio_survives_an_empty_torrent() {
        assert_eq!(TorrentAggregateStats::compute_ratio(10, 0), 0.0);
        assert_eq!(TorrentAggregateStats::compute_ratio(50, 100), 0.5);
    }

    #[test]
    fn a_large_bitfield_folds_into_a_fixed_number_of_buckets() {
        let small = TorrentPieceAvailability::from_bitfield(&vec![true; 50_000], Utc::now(), true);
        let huge =
            TorrentPieceAvailability::from_bitfield(&vec![true; 5_000_000], Utc::now(), true);
        // The payload is bounded and does not grow with the torrent.
        assert!(small.buckets.len() <= PIECE_BUCKETS);
        assert!(huge.buckets.len() <= PIECE_BUCKETS);
        assert!(small.buckets.iter().all(|bucket| *bucket == 100));
        assert_eq!(small.piece_count, 50_000);
    }

    #[test]
    fn a_small_bitfield_keeps_one_bucket_per_piece() {
        let have = vec![true, false, true, false];
        let availability = TorrentPieceAvailability::from_bitfield(&have, Utc::now(), true);
        assert_eq!(availability.buckets, vec![100, 0, 100, 0]);
        assert_eq!(availability.pieces_per_bucket, 1);
    }

    #[test]
    fn an_empty_bitfield_yields_no_buckets() {
        let availability = TorrentPieceAvailability::from_bitfield(&[], Utc::now(), false);
        assert!(availability.buckets.is_empty());
        assert!(!availability.live);
    }

    #[test]
    fn peer_addresses_are_masked_to_the_network_prefix() {
        assert_eq!(mask_peer_address("192.0.2.44:51413"), "192.0.2.x");
        assert_eq!(mask_peer_address("192.0.2.44"), "192.0.2.x");
        assert_eq!(
            mask_peer_address("[2001:db8:1:2:3:4:5:6]:6881"),
            "2001:db8:1:2::x"
        );
    }

    #[test]
    fn an_unparseable_peer_address_is_not_echoed() {
        assert_eq!(
            mask_peer_address("not-an-address"),
            crate::torrent::TRACKER_REDACTION_PLACEHOLDER
        );
    }
}
