//! The extractor's format list, normalised into something a filter can be expressed
//! against.
//!
//! yt-dlp's `formats[]` is not a stable contract: field names come and go, codec strings
//! carry profile suffixes (`avc1.640028`), and a `format_id` is rotated by the site
//! whenever it feels like it. Everything here is therefore `Option` and `#[serde(default)]`
//! on the way in, and reduced to a small closed set of enums on the way out — a filter over
//! [`VideoCodecFamily::Avc`] keeps working when the site starts emitting `avc3` tomorrow.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Version of the persisted media format blob.
/// Version 2 added the audio and subtitle track lists (RD-080-02); a version 1 blob
/// still loads, it simply carries none.
pub const MEDIA_CONTRACT_VERSION: u32 = 2;

/// Most formats kept from one probe. A page offering more is reported as truncated rather
/// than silently cut, so a filter never claims "no match" over a prefix of the inventory.
pub const MAX_MEDIA_FORMATS: usize = 200;

/// Whether a format carries video, audio, or both.
///
/// [`MediaFormatKind::Muxed`] is the distinction the whole no-ffmpeg path rests on: a
/// muxed format is downloadable on its own, a `Video`/`Audio` pair is not.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaFormatKind {
    /// Video-only stream; needs an audio stream merged onto it.
    Video,
    /// Audio-only stream.
    Audio,
    /// Progressive stream carrying both — usable without ffmpeg.
    #[default]
    Muxed,
}

/// High dynamic range signalling as reported by the extractor.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DynamicRange {
    Sdr,
    Hdr10,
    #[serde(rename = "hdr10_plus")]
    HdrPlus,
    Hlg,
    DolbyVision,
    #[default]
    Unknown,
}

impl DynamicRange {
    /// Rank used when ordering candidates; higher is richer.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Sdr => 1,
            Self::Hlg => 2,
            Self::Hdr10 => 3,
            Self::HdrPlus => 4,
            Self::DolbyVision => 5,
        }
    }

    /// Whether this is any flavour of HDR.
    #[must_use]
    pub const fn is_hdr(self) -> bool {
        matches!(
            self,
            Self::Hdr10 | Self::HdrPlus | Self::Hlg | Self::DolbyVision
        )
    }
}

/// Video codec reduced to the family a person actually chooses by.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodecFamily {
    Avc,
    Hevc,
    Av1,
    Vp9,
    Vp8,
    #[default]
    Other,
}

/// Audio codec reduced to the family a person actually chooses by.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodecFamily {
    Aac,
    Opus,
    Vorbis,
    Mp3,
    Flac,
    Ac3,
    Eac3,
    #[default]
    Other,
}

/// One normalised entry of the extractor's format list.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(default)]
pub struct MediaFormat {
    /// The extractor's own id. Volatile — shown to the user and offered as a pin, but never
    /// the thing a selection is stored as.
    pub format_id: String,
    pub kind: MediaFormatKind,
    /// Normalised container/extension (`mp4`, `webm`, `m4a`).
    pub container: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<VideoCodecFamily>,
    /// The codec string as reported, kept for display and diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec_raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<AudioCodecFamily>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec_raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    pub dynamic_range: DynamicRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bitrate_kbps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_bitrate_kbps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bitrate_kbps: Option<u32>,
    /// Lowercased language tag as reported (`de`, `en-US`); `None` when the site says nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filesize_approx: Option<u64>,
    /// Delivery protocol (`https`, `m3u8_native`, `dash`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// The extractor's free-text note (`1080p60 HDR`), display only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_note: Option<String>,
}

impl MediaFormat {
    /// Whether the format carries a video track.
    #[must_use]
    pub const fn has_video(&self) -> bool {
        matches!(self.kind, MediaFormatKind::Video | MediaFormatKind::Muxed)
    }

    /// Whether the format carries an audio track.
    #[must_use]
    pub const fn has_audio(&self) -> bool {
        matches!(self.kind, MediaFormatKind::Audio | MediaFormatKind::Muxed)
    }

    /// Bitrate used for ordering and for the `max_total_bitrate_kbps` filter, falling back
    /// to the sum of the per-track rates when the extractor reports no total.
    #[must_use]
    pub fn effective_bitrate_kbps(&self) -> Option<u32> {
        self.total_bitrate_kbps.or_else(|| {
            match (self.video_bitrate_kbps, self.audio_bitrate_kbps) {
                (None, None) => None,
                (video, audio) => Some(video.unwrap_or(0).saturating_add(audio.unwrap_or(0))),
            }
        })
    }
}

/// The formats one probe found, plus whether the list had to be cut.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(default)]
pub struct MediaFormatInventory {
    pub formats: Vec<MediaFormat>,
    /// `true` when the page offered more than [`MAX_MEDIA_FORMATS`].
    pub truncated: bool,
}

impl MediaFormatInventory {
    /// Formats of one kind.
    pub fn of_kind(&self, kind: MediaFormatKind) -> impl Iterator<Item = &MediaFormat> {
        self.formats
            .iter()
            .filter(move |format| format.kind == kind)
    }

    /// Whether anything here can be downloaded without merging two streams.
    #[must_use]
    pub fn has_muxed(&self) -> bool {
        self.formats
            .iter()
            .any(|format| format.kind == MediaFormatKind::Muxed)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.formats.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{DynamicRange, MediaFormat, MediaFormatKind};

    #[test]
    fn dynamic_range_ranks_hdr_above_sdr() {
        assert!(DynamicRange::Hdr10.rank() > DynamicRange::Sdr.rank());
        assert!(DynamicRange::Sdr.rank() > DynamicRange::Unknown.rank());
        assert!(DynamicRange::Hdr10.is_hdr());
        assert!(!DynamicRange::Sdr.is_hdr());
    }

    #[test]
    fn effective_bitrate_falls_back_to_the_track_sum() {
        let format = MediaFormat {
            kind: MediaFormatKind::Video,
            video_bitrate_kbps: Some(4_000),
            audio_bitrate_kbps: Some(128),
            ..MediaFormat::default()
        };
        assert_eq!(format.effective_bitrate_kbps(), Some(4_128));

        let total = MediaFormat {
            total_bitrate_kbps: Some(5_000),
            ..format
        };
        assert_eq!(total.effective_bitrate_kbps(), Some(5_000));
        assert_eq!(MediaFormat::default().effective_bitrate_kbps(), None);
    }
}
