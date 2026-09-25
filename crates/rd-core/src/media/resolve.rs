//! What resolving criteria against an inventory produced — including, when nothing matched,
//! enough detail to explain *why* to a person.
//!
//! "No formats match" is a useless thing to show someone who just ticked AV1 and HDR on a
//! page that offers both, but never together. [`MediaResolution::matched_counts`] therefore
//! carries, per criterion in isolation, how many formats that one criterion alone would
//! have kept, which is exactly what the UI needs to say "AV1 alone matches 6, HDR alone
//! matches 2, together none".
//!
//! The algorithm itself lives in `rd-media`, because it needs to know whether ffmpeg is
//! present; only the vocabulary is shared here.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::format::MediaFormat;

/// One filter of [`super::MediaFormatCriteria`], named so it can be reported and relaxed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CriterionKind {
    DynamicRange,
    VideoCodec,
    AudioCodec,
    Container,
    Fps,
    Bitrate,
    Language,
    Height,
}

impl CriterionKind {
    /// Stable identifier used as the i18n key suffix in the UI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DynamicRange => "dynamic_range",
            Self::VideoCodec => "video_codec",
            Self::AudioCodec => "audio_codec",
            Self::Container => "container",
            Self::Fps => "fps",
            Self::Bitrate => "bitrate",
            Self::Language => "language",
            Self::Height => "height",
        }
    }
}

/// The order criteria are dropped in when nothing matches and the selection is
/// [`super::MediaStrictness::Preferred`].
///
/// Deliberately a constant rather than a chain of `if`s: the property test asserts that
/// relaxing is monotone against exactly this array, which is only meaningful if there is
/// one place that decides the order. Cosmetic preferences go first, and the height bound
/// goes last because it is the one people mean most literally.
pub const RELAXATION_ORDER: [CriterionKind; 8] = [
    CriterionKind::DynamicRange,
    CriterionKind::VideoCodec,
    CriterionKind::AudioCodec,
    CriterionKind::Container,
    CriterionKind::Fps,
    CriterionKind::Bitrate,
    CriterionKind::Language,
    CriterionKind::Height,
];

/// How many formats one criterion would keep on its own.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CriterionMatch {
    pub criterion: CriterionKind,
    /// Formats matching this criterion alone, ignoring every other one.
    pub matched: usize,
}

/// Something the user asked for that the result does not honour.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MediaCompatibilityWarning {
    /// ffmpeg is missing or incomplete, so only progressive formats are usable.
    MergeUnavailable,
    /// The chosen codec cannot live in the chosen container.
    CodecContainerMismatch { codec: String, container: String },
    /// The target container cannot hold what was asked of it.
    ContainerUnsupported { container: String },
    /// The inventory was cut at [`super::MAX_MEDIA_FORMATS`].
    InventoryTruncated,
}

/// What the resolver settled on.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
pub struct MediaResolution {
    /// The primary format: video, muxed, or — for an audio-only target — the audio stream.
    pub video: Option<MediaFormat>,
    /// The separate audio format, when the result is a merge.
    pub audio: Option<MediaFormat>,
    /// The yt-dlp `-f` expression, pinned id first and a merge-free preset last.
    pub format_expression: String,
    /// Container of the finished file.
    pub container: String,
    pub estimated_bytes: Option<u64>,
    /// Criteria that had to be dropped to get any result, in the order they were dropped.
    pub relaxations: Vec<CriterionKind>,
    pub warnings: Vec<MediaCompatibilityWarning>,
    /// Per-criterion isolated match counts, for explaining the outcome.
    pub matched_counts: Vec<CriterionMatch>,
    /// Formats left after all criteria were applied.
    pub matched_total: usize,
    /// Formats considered before filtering.
    pub candidate_total: usize,
}

impl MediaResolution {
    /// Whether the result honours everything that was asked for.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.relaxations.is_empty()
    }
}

/// Why no format could be chosen.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum MediaSelectionError {
    /// The page offered nothing at all, or nothing of the requested kind.
    #[error("the page offers no usable media format")]
    NoFormats,
    /// Criteria were marked required and together match nothing.
    #[error("no format satisfies every required criterion")]
    NoMatch {
        /// Criteria that keep nothing even on their own — the ones actually to blame.
        unsatisfiable: Vec<CriterionKind>,
        matched_counts: Vec<CriterionMatch>,
        candidate_total: usize,
    },
    /// A video was requested, ffmpeg is unavailable, and the page has no progressive format.
    #[error("the page offers no progressive format and ffmpeg is unavailable for merging")]
    MergeRequired,
    /// Only video streams exist and not one audio stream to merge onto them, with or
    /// without ffmpeg (RD-120-50). Kept apart from [`Self::MergeRequired`], whose message
    /// blames a missing ffmpeg that may well be installed.
    #[error("the page offers video streams but no audio stream to merge onto them")]
    NoAudio,
}

impl MediaSelectionError {
    /// The stable REST error code the frontend translates.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NoFormats => "media.formats_missing",
            Self::NoMatch { .. } => "media.criteria_unsatisfiable",
            Self::MergeRequired => "media.merge_unavailable",
            Self::NoAudio => "media.audio_missing",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CriterionKind, MediaSelectionError, RELAXATION_ORDER};

    #[test]
    fn relaxation_order_covers_every_criterion_exactly_once() {
        let mut seen = RELAXATION_ORDER.to_vec();
        seen.sort_by_key(|kind| kind.as_str());
        seen.dedup();
        assert_eq!(seen.len(), RELAXATION_ORDER.len());
        // Height is what people mean most literally, so it is relaxed last.
        assert_eq!(RELAXATION_ORDER.last(), Some(&CriterionKind::Height));
    }

    #[test]
    fn every_error_carries_a_stable_code() {
        assert_eq!(
            MediaSelectionError::NoFormats.code(),
            "media.formats_missing"
        );
        assert_eq!(
            MediaSelectionError::MergeRequired.code(),
            "media.merge_unavailable"
        );
        assert_eq!(MediaSelectionError::NoAudio.code(), "media.audio_missing");
    }
}
