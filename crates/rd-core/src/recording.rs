//! What a livestream recording produces besides the video (RD-080-09).
//!
//! A long recording is not one file written once. It is a sequence of attempts across
//! disconnects, optionally cut into segments, with sidecars beside it and a remux at the end.
//! Three rules shape the types below:
//!
//! * **Already-recorded bytes are never discarded.** A disconnect ends a segment; it does not
//!   end the recording, and it must not cost what was already on disk.
//! * **Segment names sort in the order they were recorded.** A viewer concatenating them, and
//!   the remux step joining them, both depend on that ordering being lexical.
//! * **Nothing claims to have captured what the provider did not offer.** A sidecar that was
//!   asked for and is unavailable is recorded as unavailable, not silently skipped.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Shortest split interval, in minutes. Below this the segments cost more than they help.
pub const MIN_SPLIT_MINUTES: u32 = 1;
/// Longest split interval.
pub const MAX_SPLIT_MINUTES: u32 = 24 * 60;
/// Smallest split size, in mebibytes.
pub const MIN_SPLIT_MEGABYTES: u64 = 16;
/// Most reconnect attempts before a recording is given up on.
pub const MAX_RECONNECTS: u32 = 100;
/// Most segments one recording may produce.
pub const MAX_SEGMENTS: u32 = 1_000;

/// How a long recording is cut up.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "mode", content = "value")]
pub enum SplitPolicy {
    /// One file, however long it runs. A disconnect still starts a new segment — that is
    /// recovery, not splitting.
    #[default]
    None,
    /// Cut after this many minutes.
    Duration(u32),
    /// Cut after this many mebibytes.
    Size(u64),
}

impl SplitPolicy {
    /// Whether a segment that has run `elapsed_seconds` and holds `bytes` should be cut.
    ///
    /// Deliberately a pure decision over two numbers: the cases worth testing are boundaries,
    /// and none of them are worth reproducing by recording for an hour.
    #[must_use]
    pub const fn should_split(self, elapsed_seconds: u64, bytes: u64) -> bool {
        match self {
            Self::None => false,
            Self::Duration(minutes) => elapsed_seconds >= (minutes as u64) * 60,
            Self::Size(megabytes) => bytes >= megabytes * 1024 * 1024,
        }
    }

    /// Whether the policy is expressible; a zero bound would cut on every tick.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        match self {
            Self::None => true,
            Self::Duration(minutes) => minutes >= MIN_SPLIT_MINUTES && minutes <= MAX_SPLIT_MINUTES,
            Self::Size(megabytes) => megabytes >= MIN_SPLIT_MEGABYTES,
        }
    }
}

/// What the finished recording is converted to.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemuxTarget {
    /// Keep the MPEG-TS streamlink wrote. Nothing to do, and nothing that can fail.
    #[default]
    None,
    Mkv,
    Mp4,
}

impl RemuxTarget {
    /// Container extension, or `None` when no remux is wanted.
    #[must_use]
    pub const fn extension(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Mkv => Some("mkv"),
            Self::Mp4 => Some("mp4"),
        }
    }
}

/// Sidecars to capture beside the recording.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct SidecarPolicy {
    /// The stream's metadata as the provider reported it, written as JSON.
    pub metadata: bool,
    /// The channel's thumbnail, where the provider names one.
    pub thumbnail: bool,
    /// Subtitles, where the provider carries them as a separate track.
    pub subtitles: bool,
    /// Live chat. Requires a provider-specific client, which is why it can be *asked* for and
    /// still be reported unavailable.
    pub chat: bool,
}

impl SidecarPolicy {
    /// Whether anything at all was asked for.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.metadata && !self.thumbnail && !self.subtitles && !self.chat
    }
}

/// Why a requested sidecar was not produced.
///
/// Recorded rather than silently dropped: "I asked for chat and there is none" has to be
/// distinguishable from "the recording forgot".
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SidecarStatus {
    /// Written to disk.
    Captured,
    /// The provider does not expose it for this stream.
    NotOffered,
    /// rDownloader cannot capture this kind for this provider.
    Unsupported,
    /// It was offered but fetching it failed.
    Failed,
}

/// One sidecar's outcome.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SidecarOutcome {
    /// `metadata`, `thumbnail`, `subtitles`, `chat`.
    pub kind: String,
    pub status: SidecarStatus,
    /// File name beside the recording, when one was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

/// When an incomplete live recording may be replaced by the published VOD.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "mode", content = "value")]
pub enum VodFallback {
    /// Never. The default, because fetching a VOD doubles the traffic and the recording is
    /// usually fine.
    #[default]
    Off,
    /// Fetch the VOD when the live recording covered less than this percentage of the
    /// scheduled window.
    BelowCoverage(u8),
}

impl VodFallback {
    /// Whether a recording covering `recorded_seconds` of a `window_seconds` window is
    /// incomplete enough to warrant the VOD.
    ///
    /// A window of zero — an unscheduled recording — never triggers it: without an expected
    /// length there is nothing to be short of, and guessing would re-download every stream.
    #[must_use]
    pub fn should_fetch(self, recorded_seconds: u64, window_seconds: u64) -> bool {
        match self {
            Self::Off => false,
            Self::BelowCoverage(percent) => {
                if window_seconds == 0 {
                    return false;
                }
                let covered = recorded_seconds.saturating_mul(100) / window_seconds;
                covered < u64::from(percent)
            }
        }
    }
}

/// Everything RD-080-09 adds to one recording.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct RecordingPolicy {
    pub split: SplitPolicy,
    pub remux: RemuxTarget,
    pub sidecars: SidecarPolicy,
    pub vod_fallback: VodFallback,
    /// Seconds to wait before reconnecting after an unexpected end.
    pub reconnect_delay_seconds: u32,
}

impl RecordingPolicy {
    /// Whether every bound is expressible.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.split.is_valid() && self.reconnect_delay_seconds <= 600
    }
}

/// One continuous stretch of recording.
///
/// A new segment starts when the split policy says so *or* when a disconnect ends the last
/// one. Both are the same event as far as the files are concerned; only `reason` differs, and
/// it is kept because "this recording has a gap in it" is something a person needs to know.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct RecordingSegment {
    /// 1-based, matching the number in the file name.
    pub index: u32,
    pub file_name: String,
    pub bytes: u64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub reason: SegmentEnd,
}

/// Why a segment ended.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SegmentEnd {
    /// Still recording.
    #[default]
    Open,
    /// The split policy cut it. No gap: the next segment starts immediately.
    Split,
    /// The stream dropped. There *is* a gap, however short.
    Disconnect,
    /// The stream ended, or the user stopped it.
    Finished,
}

impl SegmentEnd {
    /// Whether the transition left a hole in the recording.
    #[must_use]
    pub const fn leaves_gap(self) -> bool {
        matches!(self, Self::Disconnect)
    }
}

/// The persisted state of one recording (RD-080-09).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct RecordingState {
    pub segments: Vec<RecordingSegment>,
    pub sidecars: Vec<SidecarOutcome>,
    /// How many times the stream had to be reconnected.
    pub reconnects: u32,
    /// Whether a VOD was fetched to fill in for an incomplete recording.
    pub vod_fetched: bool,
}

impl RecordingState {
    /// Total bytes across every segment.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.segments.iter().map(|segment| segment.bytes).sum()
    }

    /// Whether any segment boundary left a gap.
    #[must_use]
    pub fn has_gaps(&self) -> bool {
        self.segments
            .iter()
            .any(|segment| segment.reason.leaves_gap())
    }

    /// Seconds actually covered, summed over the segments that have ended.
    #[must_use]
    pub fn recorded_seconds(&self) -> u64 {
        self.segments
            .iter()
            .filter_map(|segment| {
                let end = segment.ended_at?;
                u64::try_from((end - segment.started_at).num_seconds()).ok()
            })
            .sum()
    }
}

/// The file name of one segment.
///
/// Zero-padded so the names sort in recording order: `show.part002.ts` after
/// `show.part001.ts`, and still correct at `part010`. A viewer concatenating them and the
/// remux step joining them both depend on that, and an unpadded `part10` would sort between
/// `part1` and `part2`.
#[must_use]
pub fn segment_name(stem: &str, index: u32, extension: &str) -> String {
    format!("{stem}.part{index:03}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::{
        RecordingSegment, RecordingState, RemuxTarget, SegmentEnd, SplitPolicy, VodFallback,
        segment_name,
    };
    use chrono::{Duration, Utc};

    #[test]
    fn segment_names_sort_in_recording_order() {
        // The bug this prevents: an unpadded `part10` sorts between `part1` and `part2`, and
        // the remux would join the segments in the wrong order.
        let mut names: Vec<String> = (1..=12)
            .map(|index| segment_name("show", index, "ts"))
            .collect();
        let recorded = names.clone();
        names.sort();
        assert_eq!(names, recorded);
        assert_eq!(recorded[0], "show.part001.ts");
        assert_eq!(recorded[11], "show.part012.ts");
    }

    #[test]
    fn a_duration_split_cuts_at_its_boundary_and_not_before() {
        let policy = SplitPolicy::Duration(30);
        assert!(!policy.should_split(29 * 60, u64::MAX));
        assert!(policy.should_split(30 * 60, 0));
    }

    #[test]
    fn a_size_split_cuts_at_its_boundary_and_not_before() {
        let policy = SplitPolicy::Size(100);
        assert!(!policy.should_split(u64::MAX, 100 * 1024 * 1024 - 1));
        assert!(policy.should_split(0, 100 * 1024 * 1024));
    }

    #[test]
    fn no_split_policy_never_cuts() {
        assert!(!SplitPolicy::None.should_split(u64::MAX, u64::MAX));
    }

    #[test]
    fn a_bound_that_would_cut_on_every_tick_is_invalid() {
        assert!(!SplitPolicy::Duration(0).is_valid());
        assert!(!SplitPolicy::Size(0).is_valid());
        assert!(SplitPolicy::None.is_valid());
        assert!(SplitPolicy::Duration(30).is_valid());
        assert!(SplitPolicy::Size(100).is_valid());
    }

    fn segment(index: u32, seconds: i64, bytes: u64, reason: SegmentEnd) -> RecordingSegment {
        let started = Utc::now();
        RecordingSegment {
            index,
            file_name: segment_name("show", index, "ts"),
            bytes,
            started_at: started,
            ended_at: Some(started + Duration::seconds(seconds)),
            reason,
        }
    }

    #[test]
    fn a_disconnect_is_a_gap_and_a_split_is_not() {
        // The distinction people actually care about: one means missing footage, the other
        // means the file got long.
        let split = RecordingState {
            segments: vec![
                segment(1, 60, 10, SegmentEnd::Split),
                segment(2, 60, 10, SegmentEnd::Finished),
            ],
            ..RecordingState::default()
        };
        assert!(!split.has_gaps());

        let dropped = RecordingState {
            segments: vec![
                segment(1, 60, 10, SegmentEnd::Disconnect),
                segment(2, 60, 10, SegmentEnd::Finished),
            ],
            ..RecordingState::default()
        };
        assert!(dropped.has_gaps());
    }

    #[test]
    fn totals_are_summed_across_segments() {
        let state = RecordingState {
            segments: vec![
                segment(1, 120, 1_000, SegmentEnd::Disconnect),
                segment(2, 60, 2_000, SegmentEnd::Finished),
            ],
            ..RecordingState::default()
        };
        assert_eq!(state.total_bytes(), 3_000);
        assert_eq!(state.recorded_seconds(), 180);
    }

    #[test]
    fn a_still_open_segment_contributes_bytes_but_no_duration() {
        // It has no end yet, so its length is unknown; counting it as zero is honest, and
        // counting "now" would make the number move every time it is read.
        let started = Utc::now();
        let state = RecordingState {
            segments: vec![RecordingSegment {
                index: 1,
                file_name: "show.part001.ts".to_owned(),
                bytes: 500,
                started_at: started,
                ended_at: None,
                reason: SegmentEnd::Open,
            }],
            ..RecordingState::default()
        };
        assert_eq!(state.total_bytes(), 500);
        assert_eq!(state.recorded_seconds(), 0);
    }

    #[test]
    fn the_vod_fallback_is_off_unless_the_recording_fell_short() {
        let policy = VodFallback::BelowCoverage(90);
        // Two hours scheduled, one recorded: half, well under the threshold.
        assert!(policy.should_fetch(3_600, 7_200));
        // Nearly all of it: not worth doubling the traffic.
        assert!(!policy.should_fetch(7_000, 7_200));
        assert!(!policy.should_fetch(7_200, 7_200));
    }

    #[test]
    fn an_unscheduled_recording_never_triggers_the_vod_fallback() {
        // Without an expected length there is nothing to fall short of, and guessing would
        // re-download every stream that was ever recorded.
        assert!(!VodFallback::BelowCoverage(90).should_fetch(10, 0));
        assert!(!VodFallback::Off.should_fetch(0, 7_200));
    }

    #[test]
    fn remux_targets_name_their_container() {
        assert_eq!(RemuxTarget::None.extension(), None);
        assert_eq!(RemuxTarget::Mkv.extension(), Some("mkv"));
        assert_eq!(RemuxTarget::Mp4.extension(), Some("mp4"));
    }
}
