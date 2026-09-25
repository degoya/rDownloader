//! REST contract for the media format selector (RD-080-01).
//!
//! Kept out of `dto.rs` deliberately, the way `replay_dto` is: the selector's shapes are
//! large, they are only used by three endpoints, and nothing else in the settings contract
//! needs to know about them.

use rd_core::{
    AudioTrack, CriterionMatch, EmbedWarning, MediaCompatibilityWarning, MediaFormatCriteria,
    MediaFormatInventory, MediaVariant, SubtitleTrack, TrackWarning,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What the tools on this installation allow, so the UI can grey out what cannot work
/// instead of re-deriving the rule.
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaToolCapabilities {
    /// ffmpeg *and* ffprobe are present: separate video and audio streams can be merged.
    pub can_merge: bool,
    /// Audio can be extracted and re-encoded.
    pub can_transcode_audio: bool,
}

/// The full inventory behind one media candidate, fetched only when the selector is opened.
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaFormatsResponse {
    pub inventory: MediaFormatInventory,
    /// The criteria currently stored for this candidate.
    pub criteria: MediaFormatCriteria,
    /// What those criteria resolve to right now.
    pub resolved: Option<MediaResolutionResponse>,
    /// Why `resolved` is empty, as the resolver's stable code (`media.formats_missing`,
    /// `media.audio_missing`, `media.merge_unavailable`, `media.criteria_unsatisfiable`),
    /// so the selector names the actual reason rather than guessing one (RD-120-50).
    pub unresolved_code: Option<String>,
    pub capabilities: MediaToolCapabilities,
    /// Selectable audio tracks (RD-080-02).
    pub audio_tracks: Vec<AudioTrack>,
    /// Subtitle tracks the page offers, manual and automatic kept apart (RD-080-02).
    pub subtitles: Vec<SubtitleTrack>,
}

/// A template to preview against one candidate's real metadata.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MediaOutputPreviewRequest {
    /// The template to expand. Empty previews the plain file name.
    pub template: String,
}

/// What a template expands to for one candidate.
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaOutputPreviewResponse {
    /// The path relative to the package directory, using `/` separators.
    pub relative_path: String,
    /// The fields a template may reference, so the UI never has to hard-code them.
    pub fields: Vec<String>,
}

/// A selection to apply, or to preview without applying.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MediaSelectionRequest {
    /// A preset id (`best`, `1080p`, `audio_mp3`). Ignored when `criteria` is given.
    #[serde(default)]
    pub preset: Option<String>,
    /// The full criteria. Wins over `preset`.
    #[serde(default)]
    pub criteria: Option<MediaFormatCriteria>,
}

/// What a set of criteria resolves to.
#[derive(Debug, Serialize, ToSchema)]
pub struct MediaResolutionResponse {
    /// The yt-dlp expression that would be used. Diagnostic only.
    pub format_expression: String,
    pub container: String,
    pub estimated_bytes: Option<u64>,
    /// Human-readable description of the chosen format (`AV1 1080p60 HDR`).
    pub label: String,
    /// Criteria that had to be dropped, as stable keys the UI translates.
    pub relaxations: Vec<String>,
    pub warnings: Vec<MediaCompatibilityWarning>,
    /// How many formats each criterion would keep on its own — what makes an empty result
    /// explainable rather than merely reported.
    pub matched_counts: Vec<CriterionMatch>,
    pub matched_total: usize,
    pub candidate_total: usize,
    /// The variant this selection would store on the candidate.
    pub variant: MediaVariant,
    /// What the requested tracks cannot honour (RD-080-02). Warnings, not refusals: a
    /// missing subtitle language must not stop a download that is otherwise right.
    pub track_warnings: Vec<TrackWarning>,
    /// What the embed policy cannot honour (RD-080-03), including a source URL withheld
    /// because it carries a signature.
    pub embed_warnings: Vec<EmbedWarning>,
}

/// Which cookie/authentication profile a link should be queued with (RD-080-04).
///
/// Mirrors `AuthProfileSelection`'s three states rather than a nullable id, because
/// "let the scope decide" and "deliberately send nothing" are different intents.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CandidateAuthProfileRequest {
    /// `auto`, `none`, or `pinned` together with `profile_id`.
    pub mode: CandidateAuthProfileMode,
    /// Required for `pinned`, ignored otherwise.
    #[serde(default)]
    pub profile_id: Option<rd_core::AuthProfileId>,
}

/// The three ways a link can choose a profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateAuthProfileMode {
    /// Apply the most specific enabled profile whose scope matches the URL.
    Auto,
    /// Send no profile at all, even if one would match.
    None,
    /// Use exactly the profile named by `profile_id`.
    Pinned,
}
