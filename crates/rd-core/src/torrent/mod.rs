//! Torrent domain contracts: settings, parsed metadata, the per-file download plan,
//! seeding policy, live statistics and engine capabilities.

mod capabilities;
mod metadata;
mod plan;
mod policy;
mod settings;
mod state;
mod stats;

pub use capabilities::TorrentEngineCapabilities;
pub use metadata::{
    MAX_TORRENT_TRACKERS, MAX_TRACKER_URL, TRACKER_REDACTION_PLACEHOLDER, TorrentFileEntry,
    TorrentMetadataInfo, TorrentTracker, TrackerOrigin, TrackerScrape, redact_tracker_url,
    tracker_id,
};
pub use plan::{
    MAX_EXCLUSION_PATTERN_LENGTH, MAX_EXCLUSION_PATTERNS, ResolvedTorrentPlan, TorrentFileDecision,
    TorrentFilePlan, TorrentFilePriority, TorrentSequentialMode, glob_match, resolve_plan,
};
pub use policy::{
    EffectiveSeedingPolicy, MAX_SEED_RATIO, MAX_SEED_TIME_MINUTES, MIN_SEED_RATIO, PolicySource,
    SeedTimeLimit, SeedingPolicyOverride, resolve_seeding_policy,
};
pub use settings::{TORRENT_CONTENT_TYPES, TORRENT_PROVIDER, TorrentListenMode, TorrentSettings};
pub use state::{
    SeedAccounting, TORRENT_CONTRACT_VERSION, TorrentCandidateState, TorrentCandidateSummary,
    TorrentJobState, TorrentMetadataState,
};
pub use stats::{
    DEFAULT_PEER_PAGE, MAX_PEER_PAGE, PIECE_BUCKETS, TorrentAggregateStats, TorrentPeerEntry,
    TorrentPeerPage, TorrentPieceAvailability, mask_peer_address,
};
