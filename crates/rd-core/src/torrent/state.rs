//! The torrent state rDownloader persists next to a link candidate and a queue row.
//!
//! Both are typed JSON blobs, following the `media_json` / `request_json` precedent: the
//! blob carries a [`TORRENT_CONTRACT_VERSION`] so a future format change is detectable
//! instead of silently misreading old rows.

use serde::{Deserialize, Serialize};

use super::{
    metadata::TorrentMetadataInfo,
    plan::{TorrentFilePlan, TorrentFilePriority},
    policy::SeedingPolicyOverride,
};

/// Version of the persisted torrent blobs.
pub const TORRENT_CONTRACT_VERSION: u32 = 1;

/// Whether the metadata behind a candidate is known yet. Magnets start out `Pending` and
/// become `Ready` once the engine has resolved the info dictionary.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TorrentMetadataState {
    #[default]
    Pending,
    Ready,
    Failed,
}

/// Torrent state stored on a link candidate, i.e. before the torrent is queued.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TorrentCandidateState {
    pub contract_version: u32,
    pub metadata_state: TorrentMetadataState,
    pub metadata: Option<TorrentMetadataInfo>,
    /// Why metadata resolution failed, redaction-safe.
    pub metadata_error: Option<String>,
    pub plan: TorrentFilePlan,
}

impl Default for TorrentCandidateState {
    fn default() -> Self {
        Self {
            contract_version: TORRENT_CONTRACT_VERSION,
            metadata_state: TorrentMetadataState::Pending,
            metadata: None,
            metadata_error: None,
            plan: TorrentFilePlan::default(),
        }
    }
}

impl TorrentCandidateState {
    /// Candidate state for a torrent whose metadata is already known.
    #[must_use]
    pub fn ready(metadata: TorrentMetadataInfo) -> Self {
        Self {
            metadata_state: TorrentMetadataState::Ready,
            metadata: Some(metadata),
            ..Self::default()
        }
    }

    /// Whether the blob was written by a newer version of rDownloader.
    #[must_use]
    pub fn is_future_contract(&self) -> bool {
        self.contract_version > TORRENT_CONTRACT_VERSION
    }

    /// The list-sized view of this state.
    #[must_use]
    pub fn summary(&self) -> TorrentCandidateSummary {
        let Some(metadata) = self.metadata.as_ref() else {
            return TorrentCandidateSummary {
                metadata_state: self.metadata_state,
                file_count: 0,
                selected_count: 0,
                selected_bytes: crate::ByteCount::default(),
                total_bytes: crate::ByteCount::default(),
                has_exclusions: !self.plan.exclusion_patterns.is_empty(),
            };
        };
        let resolved = super::plan::resolve_plan(metadata, &self.plan);
        let selected_count = resolved.files.iter().filter(|file| file.included).count();
        TorrentCandidateSummary {
            metadata_state: self.metadata_state,
            file_count: u32::try_from(metadata.files.len()).unwrap_or(u32::MAX),
            selected_count: u32::try_from(selected_count).unwrap_or(u32::MAX),
            selected_bytes: resolved.selected_bytes,
            total_bytes: resolved.total_bytes,
            has_exclusions: !self.plan.exclusion_patterns.is_empty(),
        }
    }
}

/// Bounded summary of a candidate's torrent state.
///
/// The full file tree can hold thousands of entries, which would bloat every LinkGrabber
/// list response. Lists carry this summary; the tree itself is fetched per candidate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, utoipa::ToSchema)]
pub struct TorrentCandidateSummary {
    pub metadata_state: TorrentMetadataState,
    pub file_count: u32,
    pub selected_count: u32,
    pub selected_bytes: crate::ByteCount,
    pub total_bytes: crate::ByteCount,
    pub has_exclusions: bool,
}

/// How long a torrent has been seeding, across restarts.
///
/// `std::time::Instant` cannot survive a process restart, which is why the seed clock used
/// to reset on every start. Persisting a wall-clock start plus the already accumulated
/// duration keeps the elapsed time correct over any number of restarts.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct SeedAccounting {
    /// Start of the current seeding stretch; `None` while not seeding.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Seconds seeded in earlier stretches.
    pub accumulated_seconds: u64,
}

impl SeedAccounting {
    /// Total seconds seeded, including the currently running stretch.
    #[must_use]
    pub fn seeded_seconds(&self, now: chrono::DateTime<chrono::Utc>) -> u64 {
        let running = self
            .started_at
            .map(|started| (now - started).num_seconds().max(0) as u64)
            .unwrap_or_default();
        self.accumulated_seconds.saturating_add(running)
    }

    /// Opens a new seeding stretch, keeping what was already accumulated.
    pub fn start(&mut self, now: chrono::DateTime<chrono::Utc>) {
        if self.started_at.is_none() {
            self.started_at = Some(now);
        }
    }

    /// Closes the running stretch and folds it into the accumulated total.
    pub fn stop(&mut self, now: chrono::DateTime<chrono::Utc>) {
        self.accumulated_seconds = self.seeded_seconds(now);
        self.started_at = None;
    }
}

/// Torrent state stored on a queue row.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TorrentJobState {
    pub contract_version: u32,
    pub metadata: Option<TorrentMetadataInfo>,
    pub plan: TorrentFilePlan,
    /// Lowest priority tier the emulation has opened so far; `None` means "not started".
    /// Persisted so the staging survives a restart instead of beginning again at the top.
    pub open_tier: Option<TorrentFilePriority>,
    pub seeding: SeedingPolicyOverride,
    pub seed: SeedAccounting,
}

impl Default for TorrentJobState {
    fn default() -> Self {
        Self {
            contract_version: TORRENT_CONTRACT_VERSION,
            metadata: None,
            plan: TorrentFilePlan::default(),
            open_tier: None,
            seeding: SeedingPolicyOverride::default(),
            seed: SeedAccounting::default(),
        }
    }
}

impl TorrentJobState {
    /// Carries a reviewed candidate state over to the queue row.
    #[must_use]
    pub fn from_candidate(candidate: TorrentCandidateState) -> Self {
        Self {
            metadata: candidate.metadata,
            plan: candidate.plan,
            ..Self::default()
        }
    }

    /// Whether the blob was written by a newer version of rDownloader.
    #[must_use]
    pub fn is_future_contract(&self) -> bool {
        self.contract_version > TORRENT_CONTRACT_VERSION
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{SeedAccounting, TorrentCandidateState, TorrentJobState, TorrentMetadataState};

    #[test]
    fn a_default_candidate_waits_for_metadata() {
        let state = TorrentCandidateState::default();
        assert_eq!(state.metadata_state, TorrentMetadataState::Pending);
        assert!(state.plan.is_untouched());
        assert!(!state.is_future_contract());
    }

    #[test]
    fn seed_time_survives_a_restart() {
        let start = Utc.timestamp_opt(1_000, 0).single().expect("valid");
        let mut accounting = SeedAccounting::default();
        accounting.start(start);
        // First stretch: 10 minutes, then the process stops.
        accounting.stop(start + chrono::Duration::minutes(10));
        assert_eq!(accounting.accumulated_seconds, 600);
        assert!(accounting.started_at.is_none());

        // After the restart the clock continues instead of resetting.
        let resume = start + chrono::Duration::hours(5);
        accounting.start(resume);
        assert_eq!(
            accounting.seeded_seconds(resume + chrono::Duration::minutes(5)),
            900
        );
    }

    #[test]
    fn starting_twice_does_not_restart_the_stretch() {
        let start = Utc.timestamp_opt(1_000, 0).single().expect("valid");
        let mut accounting = SeedAccounting::default();
        accounting.start(start);
        accounting.start(start + chrono::Duration::minutes(3));
        assert_eq!(
            accounting.seeded_seconds(start + chrono::Duration::minutes(4)),
            240
        );
    }

    #[test]
    fn a_blob_from_a_newer_version_is_detectable() {
        let json = serde_json::json!({ "contract_version": 99 });
        let state: TorrentJobState = serde_json::from_value(json).expect("defaults fill in");
        assert!(state.is_future_contract());
    }

    #[test]
    fn an_empty_blob_deserializes_to_the_defaults() {
        let state: TorrentJobState =
            serde_json::from_value(serde_json::json!({})).expect("defaults fill in");
        assert!(state.metadata.is_none());
        assert!(state.seeding.is_empty());
        assert_eq!(state.seed.accumulated_seconds, 0);
    }
}
