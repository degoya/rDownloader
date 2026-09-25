//! Media (video/audio) downloads resolved by an external extractor such as yt-dlp:
//! what a page offers, which variant the user picked, and the tool settings.
//!
//! [`MediaVariant`] is the short, presentable list shown next to a link. The full
//! normalised inventory behind it lives in [`format`], and what the user actually chose is
//! stored as [`MediaFormatCriteria`] rather than as a format id — see [`criteria`] for why.

mod container;
mod criteria;
mod embed;
mod format;
mod resolve;
mod tracks;

use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

pub use container::{
    CONTAINERS, ContainerCapabilities, capabilities, is_audio_only, supports_audio_codec,
    supports_chapters, supports_multiple_audio, supports_subtitles, supports_thumbnail,
    supports_video_codec,
};
pub use criteria::{
    CriteriaError, LEGACY_PRESETS, MAX_CRITERIA_TOKEN, MAX_CRITERIA_VALUES, MediaFormatCriteria,
    MediaOutput, MediaStrictness, MediaTarget, is_criteria_token,
};
pub use embed::{
    EmbedWarning, MediaEmbedPolicy, SponsorBlockPolicy, SponsorCategory, SponsorMode,
    effective_policy, embed_warnings,
};
pub use format::{
    AudioCodecFamily, DynamicRange, MAX_MEDIA_FORMATS, MEDIA_CONTRACT_VERSION, MediaFormat,
    MediaFormatInventory, MediaFormatKind, VideoCodecFamily,
};
pub use resolve::{
    CriterionKind, CriterionMatch, MediaCompatibilityWarning, MediaResolution, MediaSelectionError,
    RELAXATION_ORDER,
};
pub use tracks::{
    AudioTrack, AudioTrackPolicy, MAX_MEDIA_TRACKS, SubtitleMode, SubtitlePolicy, SubtitleSource,
    SubtitleTrack, TrackSelection, TrackWarning, track_warnings,
};

/// Whether a variant produces a video container or an audio-only file.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    #[default]
    Video,
    Audio,
}

/// One selectable download option synthesised from the extractor's format list.
///
/// This is the *bounded* list that rides along in every LinkGrabber response; the full
/// inventory is fetched separately, following the [`crate::RemoteListingSummary`] precedent.
/// Everything after `format` is additive and defaulted, so a blob written before RD-080-01
/// still deserialises.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct MediaVariant {
    /// Stable id such as `best`, `1080p`, `720p`, `audio_mp3`, or `custom`.
    pub id: String,
    /// Human-readable label (`Video 1080p`, `Audio (MP3)`).
    pub label: String,
    pub kind: MediaKind,
    /// Container/extension of the resulting file.
    pub ext: String,
    pub height: Option<u32>,
    /// Audio bitrate in kbit/s when known.
    pub abr: Option<u32>,
    pub filesize_approx: Option<u64>,
    /// Extractor format expression (`bv*[height<=1080]+ba/b[height<=1080]`).
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    #[serde(default)]
    pub dynamic_range: DynamicRange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<VideoCodecFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<AudioCodecFamily>,
    /// Whether producing this variant needs ffmpeg to merge two streams.
    #[serde(default)]
    pub requires_merge: bool,
    /// What this variant cannot honour, if anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<MediaCompatibilityWarning>,
    /// The criteria this variant stands for; `None` for a legacy row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<MediaFormatCriteria>,
}

/// Metadata of one media page as reported by the extractor.
///
/// `upload_date`, `extractor` and `video_id` fall out of the same `yt-dlp -J` call as the
/// rest and are the fields output templates (RD-080-05) are written against.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct MediaInfo {
    pub title: String,
    pub duration_seconds: Option<u32>,
    pub uploader: Option<String>,
    pub thumbnail: Option<String>,
    #[schema(value_type = String, format = "uri")]
    pub page_url: Url,
    pub variants: Vec<MediaVariant>,
    /// Id of the selected variant (defaults to the configured preference).
    pub selected: String,
    /// Upload date as `YYYYMMDD`, exactly as the extractor reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_date: Option<String>,
    /// Extractor that handled the page (`youtube`, `ARDMediathek`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    /// The site's own id for the item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
}

impl MediaInfo {
    /// The selected variant (falls back to the first one).
    #[must_use]
    pub fn selected_variant(&self) -> Option<&MediaVariant> {
        self.variants
            .iter()
            .find(|variant| variant.id == self.selected)
            .or_else(|| self.variants.first())
    }

    /// Selection stored on the download row once the link is enqueued.
    #[must_use]
    pub fn selection(&self) -> Option<MediaSelection> {
        let variant = self.selected_variant()?;
        Some(MediaSelection {
            page_url: self.page_url.clone(),
            variant_id: variant.id.clone(),
            format: variant.format.clone(),
            kind: variant.kind,
            ext: variant.ext.clone(),
            title: self.title.clone(),
            contract_version: MEDIA_CONTRACT_VERSION,
            criteria: variant
                .criteria
                .clone()
                .or_else(|| MediaFormatCriteria::preset(&variant.id))
                .map(Box::new),
            resolved: None,
        })
    }
}

/// Version of the persisted media candidate blob.
///
/// What is stored next to a media link candidate: the normalised inventory the probe found
/// and the criteria the user picked. A typed JSON blob following the `torrent_json` and
/// `listing_json` precedent, carrying a [`MEDIA_CONTRACT_VERSION`] so a future format
/// change is detectable instead of being silently misread.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(default)]
pub struct MediaCandidateState {
    pub contract_version: u32,
    pub inventory: MediaFormatInventory,
    pub criteria: MediaFormatCriteria,
    /// Selectable audio tracks derived from the inventory (RD-080-02). Added in contract
    /// version 2; a version 1 blob simply has none, which is why every field is defaulted.
    #[serde(default)]
    pub audio_tracks: Vec<AudioTrack>,
    /// Subtitle tracks the page offers (RD-080-02).
    #[serde(default)]
    pub subtitles: Vec<SubtitleTrack>,
}

impl Default for MediaCandidateState {
    fn default() -> Self {
        Self {
            contract_version: MEDIA_CONTRACT_VERSION,
            inventory: MediaFormatInventory::default(),
            criteria: MediaFormatCriteria::default(),
            audio_tracks: Vec::new(),
            subtitles: Vec::new(),
        }
    }
}

/// One probed media item: what the LinkGrabber shows, plus the inventory behind it.
///
/// The two travel together from the probe into the database and are stored in separate
/// columns, because the inventory can hold up to [`MAX_MEDIA_FORMATS`] entries and has no
/// business riding in every candidate list response.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct MediaCandidate {
    pub info: MediaInfo,
    pub state: MediaCandidateState,
}

/// A resolved selection to store on a media candidate.
///
/// The resolving itself happens above the database — it needs the extractor-aware code in
/// `rd-media` — so what arrives here is already a decided variant plus the criteria it came
/// from.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct MediaSelectionUpdate {
    pub criteria: MediaFormatCriteria,
    pub variant: MediaVariant,
}

impl MediaCandidateState {
    /// Candidate state for a freshly probed link.
    #[must_use]
    pub fn probed(inventory: MediaFormatInventory, criteria: MediaFormatCriteria) -> Self {
        Self {
            inventory,
            criteria,
            ..Self::default()
        }
    }

    /// Whether the blob was written by a newer version of rDownloader.
    #[must_use]
    pub const fn is_future_contract(&self) -> bool {
        self.contract_version > MEDIA_CONTRACT_VERSION
    }
}

/// What the media runner needs to fetch one file.
///
/// Three generations of row live in this one type, which is why nothing is ever removed
/// from it:
///
/// * pre-RD-080-01 rows carry `contract_version: 0` and a preset `variant_id`, and are
///   understood through [`MediaFormatCriteria::preset`];
/// * selector rows carry `criteria` and re-resolve at download time;
/// * livestream recordings built by the stream monitor put a *streamlink quality string*
///   in `format` and a quality name in `variant_id`. Neither is a preset, so
///   [`MediaSelection::effective_criteria`] returns `None` for them and the runner passes
///   `format` through untouched.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct MediaSelection {
    #[schema(value_type = String, format = "uri")]
    pub page_url: Url,
    pub variant_id: String,
    /// Extractor format expression, or a streamlink quality for a recording. Kept verbatim.
    pub format: String,
    pub kind: MediaKind,
    pub ext: String,
    #[serde(default)]
    pub title: String,
    /// [`MEDIA_CONTRACT_VERSION`] of the writer; `0` for a row that predates the selector.
    #[serde(default)]
    pub contract_version: u32,
    /// The semantic selection, re-resolved before each download.
    ///
    /// Boxed because a `MediaSelection` rides inside every `DownloadFile` and every
    /// writer command that carries one; the criteria are consulted once per download, so
    /// paying an indirection there is cheaper than widening the whole queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<Box<MediaFormatCriteria>>,
    /// What the last resolve settled on. Advisory: shown in the UI and used to pin a format
    /// id as the expression's first alternative, never trusted on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Box<ResolvedFormatPlan>>,
}

/// The outcome of the last resolve, carried alongside a selection for display and pinning.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct ResolvedFormatPlan {
    /// Format ids the last resolve chose, in `-f` order.
    pub format_ids: Vec<String>,
    /// Label describing the choice (`AV1 1080p60 HDR`).
    pub label: String,
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_bytes: Option<u64>,
    /// Criteria that had to be dropped to reach this result.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relaxations: Vec<CriterionKind>,
}

impl MediaSelection {
    /// The criteria this selection stands for, or `None` when it is a recording whose
    /// `format` is a streamlink quality rather than an extractor expression.
    #[must_use]
    pub fn effective_criteria(&self) -> Option<MediaFormatCriteria> {
        self.criteria
            .as_deref()
            .cloned()
            .or_else(|| MediaFormatCriteria::preset(&self.variant_id))
    }

    /// Whether the row predates the format selector.
    #[must_use]
    pub const fn is_legacy(&self) -> bool {
        self.contract_version == 0
    }

    /// Whether the blob was written by a newer version of rDownloader.
    #[must_use]
    pub const fn is_future_contract(&self) -> bool {
        self.contract_version > MEDIA_CONTRACT_VERSION
    }
}

/// External-tool settings for media downloads (part of the `service.settings` blob, keys
/// prefixed with `media_`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct MediaSettings {
    /// Absolute path of yt-dlp; `None` = look up `yt-dlp` on `PATH`.
    pub media_ytdlp_executable: Option<String>,
    /// Absolute path of ffmpeg; `None` = look up `ffmpeg` on `PATH` (needed for MP3/merge).
    pub media_ffmpeg_executable: Option<String>,
    /// Variant id chosen for new links (`best`, `1080p`, …, `audio_mp3`). Kept as the API
    /// compatibility anchor; `media_default_criteria` wins when it is set.
    pub media_default_variant: String,
    /// Full default selection for new links. `None` means "use `media_default_variant`".
    #[serde(default)]
    pub media_default_criteria: Option<MediaFormatCriteria>,
    /// Default output template (RD-080-05); `None` keeps the plain `<file name>` layout.
    #[serde(default)]
    pub media_output_template: Option<String>,
    /// Hosts (without `www.`) handled by the media provider.
    pub media_hosts: Vec<String>,
    /// Concurrent media downloads.
    pub media_max_parallel: u32,
    /// Timeout for one metadata probe.
    pub media_check_timeout_seconds: u32,
    /// Directory searched for yt-dlp/ffmpeg/ffprobe before `PATH`; `None` = the built-in
    /// vendor folders next to the executable and in the data directory.
    pub vendor_directory: Option<String>,
}

impl MediaSettings {
    /// Common sites yt-dlp handles. Not exhaustive — yt-dlp ships ~1800 extractors whose
    /// names are not domains — but it covers what people paste in practice, and the list is
    /// editable in the settings.
    #[must_use]
    pub fn default_hosts() -> Vec<String> {
        [
            // YouTube
            "youtube.com",
            "youtu.be",
            "m.youtube.com",
            "music.youtube.com",
            // Video platforms
            "vimeo.com",
            "dailymotion.com",
            "twitch.tv",
            "rumble.com",
            "odysee.com",
            "bitchute.com",
            "peertube.tv",
            "streamable.com",
            "veoh.com",
            "dumpert.nl",
            // Social networks
            "tiktok.com",
            "instagram.com",
            "facebook.com",
            "twitter.com",
            "x.com",
            "reddit.com",
            "tumblr.com",
            "snapchat.com",
            "vk.com",
            "ok.ru",
            "bilibili.com",
            "nicovideo.jp",
            // Audio
            "soundcloud.com",
            "bandcamp.com",
            "mixcloud.com",
            "audiomack.com",
            // Public broadcasters (DE/AT/CH)
            "ardmediathek.de",
            "zdf.de",
            "arte.tv",
            "3sat.de",
            "kika.de",
            "dw.com",
            "orf.at",
            "srf.ch",
            // Public broadcasters (international)
            "bbc.co.uk",
            "channel4.com",
            "france.tv",
            "rai.it",
            "rtve.es",
            "npo.nl",
            // News and misc
            "cnn.com",
            "nytimes.com",
            "theguardian.com",
            "heise.de",
            "spiegel.de",
            "ted.com",
            "archive.org",
            "imgur.com",
            "9gag.com",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    /// Whether `host` (any casing, optional `www.`) belongs to the media provider.
    #[must_use]
    pub fn handles_host(&self, host: &str) -> bool {
        let host = host.trim_start_matches("www.").to_ascii_lowercase();
        self.media_hosts
            .iter()
            .any(|entry| host == *entry || host.ends_with(&format!(".{entry}")))
    }
}

impl Default for MediaSettings {
    fn default() -> Self {
        Self {
            media_ytdlp_executable: None,
            media_ffmpeg_executable: None,
            media_default_variant: "best".to_owned(),
            media_default_criteria: None,
            media_output_template: None,
            media_hosts: Self::default_hosts(),
            media_max_parallel: 2,
            media_check_timeout_seconds: 60,
            vendor_directory: None,
        }
    }
}

/// Provider name stored on media candidates and used for account-less enqueueing.
pub const MEDIA_PROVIDER: &str = "media";

#[cfg(test)]
mod tests {
    use super::{MEDIA_CONTRACT_VERSION, MediaKind, MediaSelection, MediaSettings, MediaTarget};

    /// A `downloads.media_json` blob exactly as rDownloader 0.6 wrote it.
    const LEGACY_SELECTION: &str = r#"{
        "page_url": "https://www.youtube.com/watch?v=abc",
        "variant_id": "1080p",
        "format": "bv*[height<=1080]+ba/b[height<=1080]",
        "kind": "video",
        "ext": "mp4",
        "title": "clip"
    }"#;

    #[test]
    fn a_pre_selector_row_still_loads_and_resolves_through_its_preset() {
        let selection: MediaSelection =
            serde_json::from_str(LEGACY_SELECTION).expect("legacy blob still deserialises");
        assert_eq!(selection.contract_version, 0);
        assert!(selection.is_legacy());
        assert!(!selection.is_future_contract());
        assert_eq!(selection.criteria, None);
        assert_eq!(selection.resolved, None);
        // The stored expression is untouched, so a row mid-download keeps working even if
        // nothing re-resolves it.
        assert_eq!(selection.format, "bv*[height<=1080]+ba/b[height<=1080]");

        let criteria = selection
            .effective_criteria()
            .expect("a preset id resolves to criteria");
        assert_eq!(criteria.max_height, Some(1080));
        assert_eq!(criteria.preset.as_deref(), Some("1080p"));
        assert_eq!(criteria.target, MediaTarget::Video);
    }

    #[test]
    fn a_recording_row_has_no_criteria_to_resolve() {
        // The stream monitor puts a streamlink quality in `format`; treating it as an
        // extractor expression would break every livestream recording.
        let selection = MediaSelection {
            page_url: "https://twitch.tv/example".parse().expect("url"),
            variant_id: "720p60".to_owned(),
            format: "720p60".to_owned(),
            kind: MediaKind::Video,
            ext: "ts".to_owned(),
            title: "stream".to_owned(),
            contract_version: MEDIA_CONTRACT_VERSION,
            criteria: None,
            resolved: None,
        };
        assert_eq!(selection.effective_criteria(), None);
    }

    #[test]
    fn a_blob_from_a_newer_version_is_detected_rather_than_misread() {
        let mut blob: serde_json::Value =
            serde_json::from_str(LEGACY_SELECTION).expect("legacy blob");
        blob["contract_version"] = serde_json::json!(MEDIA_CONTRACT_VERSION + 1);
        let selection: MediaSelection =
            serde_json::from_value(blob).expect("unknown fields default");
        assert!(selection.is_future_contract());
    }

    #[test]
    fn host_matching_ignores_www_and_case() {
        let settings = MediaSettings::default();
        assert!(settings.handles_host("www.YouTube.com"));
        assert!(settings.handles_host("youtu.be"));
        assert!(settings.handles_host("dumpert.nl"));
        assert!(!settings.handles_host("notyoutube.com"));
    }
}
