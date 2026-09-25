//! Turns yt-dlp's raw `formats[]` into the normalised inventory a filter can be expressed
//! against.
//!
//! Everything is read defensively. yt-dlp's JSON is not a stable contract: fields appear
//! and vanish between releases, codec strings carry profile suffixes (`avc1.640028`,
//! `hev1.2.4.L153.B0`), and the same page reports `vcodec: "none"` on one site and omits
//! the field on another. A missing field therefore always means "unknown", never "absent".

use rd_core::{
    AudioCodecFamily, DynamicRange, MAX_MEDIA_FORMATS, MediaFormat, MediaFormatInventory,
    MediaFormatKind, VideoCodecFamily,
};
use serde::Deserialize;

/// Subset of a yt-dlp `formats[]` entry.
///
/// Kept public and additive: the fake-yt-dlp integration fixtures build these directly.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RawFormat {
    pub format_id: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub ext: Option<String>,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    /// Audio bitrate in kbit/s.
    pub abr: Option<f64>,
    /// Video bitrate in kbit/s.
    pub vbr: Option<f64>,
    /// Total bitrate in kbit/s.
    pub tbr: Option<f64>,
    pub fps: Option<f64>,
    /// `SDR`, `HDR10`, `HDR10+`, `HLG`, `DV`.
    pub dynamic_range: Option<String>,
    pub language: Option<String>,
    /// `https`, `m3u8_native`, `dash` — read by direct manifest intake (RD-080-06).
    pub protocol: Option<String>,
    pub format_note: Option<String>,
    pub audio_channels: Option<u32>,
    /// Audio sample rate in Hz.
    pub asr: Option<u32>,
}

/// What one of yt-dlp's codec fields says about its track.
///
/// yt-dlp distinguishes the two cases itself: `"none"` is the extractor stating that the
/// track does not exist, while a `null` or missing field means nobody knows. arte.tv reports
/// its HLS audio renditions with `acodec: null`, and dumpert.nl every format with both codecs
/// `null` (RD-120-50) — reading either as "absent" hides every format such a site has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Track<'a> {
    /// `"none"`: the format carries no such track.
    Absent,
    /// `null`, missing or empty: there may well be a track, its codec is not known.
    Unknown,
    /// A codec string.
    Known(&'a str),
}

impl<'a> Track<'a> {
    fn from_field(value: Option<&'a String>) -> Self {
        match value.map(String::as_str).map(str::trim) {
            Some("none") => Track::Absent,
            None | Some("") => Track::Unknown,
            Some(codec) => Track::Known(codec),
        }
    }

    const fn present(self) -> bool {
        !matches!(self, Track::Absent)
    }

    const fn known(self) -> Option<&'a str> {
        match self {
            Track::Known(codec) => Some(codec),
            Track::Absent | Track::Unknown => None,
        }
    }
}

/// Extensions that are never downloadable media: thumbnails, storyboards and subtitle
/// tracks some extractors list among the formats (dumpert.nl lists eight, all with `null`
/// codecs). yt-dlp's own storyboards say `"none"` for both codecs and are dropped anyway.
const PSEUDO_FORMAT_EXTENSIONS: [&str; 14] = [
    "jpg", "jpeg", "png", "webp", "gif", "bmp", "avif", "mhtml", "vtt", "srt", "ass", "ssa",
    "ttml", "json",
];

impl RawFormat {
    /// Whether the entry is a picture or a text track rather than something to play.
    fn is_pseudo_format(&self) -> bool {
        self.ext.as_deref().is_some_and(|ext| {
            let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
            PSEUDO_FORMAT_EXTENSIONS.contains(&ext.as_str())
        })
    }
}

/// Maps a yt-dlp video codec string onto the family a person filters by.
///
/// Matches on the prefix before the profile suffix, so `av01.0.08M.08` and a bare `av01`
/// land in the same place.
#[must_use]
pub fn video_codec_family(codec: &str) -> VideoCodecFamily {
    let codec = codec.to_ascii_lowercase();
    let head = codec.split('.').next().unwrap_or(&codec);
    match head {
        "avc1" | "avc3" | "h264" | "x264" => VideoCodecFamily::Avc,
        "hev1" | "hvc1" | "h265" | "x265" | "hevc" => VideoCodecFamily::Hevc,
        "av01" | "av1" => VideoCodecFamily::Av1,
        "vp9" | "vp09" => VideoCodecFamily::Vp9,
        "vp8" | "vp08" => VideoCodecFamily::Vp8,
        _ => VideoCodecFamily::Other,
    }
}

/// Maps a yt-dlp audio codec string onto the family a person filters by.
#[must_use]
pub fn audio_codec_family(codec: &str) -> AudioCodecFamily {
    let codec = codec.to_ascii_lowercase();
    let head = codec.split('.').next().unwrap_or(&codec);
    match head {
        "mp4a" | "aac" => AudioCodecFamily::Aac,
        "opus" => AudioCodecFamily::Opus,
        "vorbis" => AudioCodecFamily::Vorbis,
        "mp3" => AudioCodecFamily::Mp3,
        "flac" => AudioCodecFamily::Flac,
        "ac-3" | "ac3" => AudioCodecFamily::Ac3,
        "ec-3" | "eac3" => AudioCodecFamily::Eac3,
        _ => AudioCodecFamily::Other,
    }
}

/// Maps yt-dlp's `dynamic_range` string onto the enum.
#[must_use]
pub fn dynamic_range(value: Option<&str>) -> DynamicRange {
    let Some(value) = value else {
        return DynamicRange::Unknown;
    };
    match value.trim().to_ascii_uppercase().as_str() {
        "SDR" => DynamicRange::Sdr,
        "HDR10" => DynamicRange::Hdr10,
        "HDR10+" | "HDR10PLUS" => DynamicRange::HdrPlus,
        "HLG" => DynamicRange::Hlg,
        "DV" | "DOLBYVISION" | "DOLBY VISION" => DynamicRange::DolbyVision,
        "" => DynamicRange::Unknown,
        _ => DynamicRange::Unknown,
    }
}

/// Rounds a reported bitrate to whole kbit/s, dropping nonsense values.
fn bitrate_kbps(value: Option<f64>) -> Option<u32> {
    value
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .map(|rate| rate.round() as u32)
}

/// Normalises `formats` into the inventory, capped at [`MAX_MEDIA_FORMATS`].
///
/// The kind follows yt-dlp's own reading of the codec fields: a track is missing only where
/// the field says `"none"`, so a format with both fields `null` is a muxed format whose codecs
/// nobody reported — which is also how yt-dlp's `b` treats it. Dropped are only entries that
/// state both tracks absent (storyboards) and picture or subtitle pseudo-formats: offering
/// those as choices would be a lie the selector cannot make good on.
#[must_use]
pub fn normalize(formats: &[RawFormat]) -> MediaFormatInventory {
    let mut normalized = Vec::new();
    let mut truncated = false;
    for raw in formats {
        if raw.is_pseudo_format() {
            continue;
        }
        let video_track = Track::from_field(raw.vcodec.as_ref());
        let audio_track = Track::from_field(raw.acodec.as_ref());
        let kind = match (video_track.present(), audio_track.present()) {
            (true, true) => MediaFormatKind::Muxed,
            (true, false) => MediaFormatKind::Video,
            (false, true) => MediaFormatKind::Audio,
            (false, false) => continue,
        };
        let video = video_track.known();
        let audio = audio_track.known();
        if normalized.len() >= MAX_MEDIA_FORMATS {
            truncated = true;
            break;
        }
        normalized.push(MediaFormat {
            format_id: raw.format_id.clone(),
            kind,
            container: raw
                .ext
                .as_deref()
                .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
                .unwrap_or_default(),
            video_codec: video.map(video_codec_family),
            video_codec_raw: video.map(str::to_owned),
            audio_codec: audio.map(audio_codec_family),
            audio_codec_raw: audio.map(str::to_owned),
            width: raw.width,
            height: raw.height,
            fps: raw
                .fps
                .filter(|fps| fps.is_finite() && *fps > 0.0)
                .map(|fps| fps.round() as u32),
            dynamic_range: dynamic_range(raw.dynamic_range.as_deref()),
            total_bitrate_kbps: bitrate_kbps(raw.tbr),
            video_bitrate_kbps: bitrate_kbps(raw.vbr),
            audio_bitrate_kbps: bitrate_kbps(raw.abr),
            language: raw
                .language
                .as_deref()
                .map(str::trim)
                .filter(|language| !language.is_empty() && *language != "none")
                .map(str::to_ascii_lowercase),
            filesize_approx: raw.filesize.or(raw.filesize_approx),
            protocol: raw.protocol.clone(),
            format_note: raw.format_note.clone(),
        });
    }
    MediaFormatInventory {
        formats: normalized,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use rd_core::{
        AudioCodecFamily, DynamicRange, MAX_MEDIA_FORMATS, MediaFormatKind, VideoCodecFamily,
    };

    use super::{RawFormat, audio_codec_family, normalize, video_codec_family};

    #[test]
    fn codec_families_ignore_profile_suffixes() {
        assert_eq!(video_codec_family("avc1.640028"), VideoCodecFamily::Avc);
        assert_eq!(video_codec_family("av01.0.08M.08"), VideoCodecFamily::Av1);
        assert_eq!(
            video_codec_family("hev1.2.4.L153.B0"),
            VideoCodecFamily::Hevc
        );
        assert_eq!(video_codec_family("vp09.00.10.08"), VideoCodecFamily::Vp9);
        assert_eq!(video_codec_family("theora"), VideoCodecFamily::Other);
        assert_eq!(audio_codec_family("mp4a.40.2"), AudioCodecFamily::Aac);
        assert_eq!(audio_codec_family("opus"), AudioCodecFamily::Opus);
        assert_eq!(audio_codec_family("ec-3"), AudioCodecFamily::Eac3);
    }

    #[test]
    fn kind_follows_which_codecs_are_present() {
        let formats = vec![
            RawFormat {
                format_id: "muxed".to_owned(),
                vcodec: Some("avc1.4d401f".to_owned()),
                acodec: Some("mp4a.40.2".to_owned()),
                ext: Some("MP4".to_owned()),
                ..RawFormat::default()
            },
            RawFormat {
                format_id: "video".to_owned(),
                vcodec: Some("av01.0.08M.08".to_owned()),
                acodec: Some("none".to_owned()),
                dynamic_range: Some("HDR10".to_owned()),
                fps: Some(59.94),
                ..RawFormat::default()
            },
            RawFormat {
                format_id: "audio".to_owned(),
                vcodec: Some("none".to_owned()),
                acodec: Some("opus".to_owned()),
                language: Some("DE".to_owned()),
                ..RawFormat::default()
            },
            // A storyboard pseudo-format: no codec at all, not downloadable media.
            RawFormat {
                format_id: "sb0".to_owned(),
                vcodec: Some("none".to_owned()),
                acodec: Some("none".to_owned()),
                ..RawFormat::default()
            },
        ];
        let inventory = normalize(&formats);
        assert_eq!(inventory.formats.len(), 3, "the storyboard is dropped");
        assert_eq!(inventory.formats[0].kind, MediaFormatKind::Muxed);
        assert_eq!(inventory.formats[0].container, "mp4", "lowercased");
        assert_eq!(inventory.formats[1].kind, MediaFormatKind::Video);
        assert_eq!(inventory.formats[1].dynamic_range, DynamicRange::Hdr10);
        assert_eq!(inventory.formats[1].fps, Some(60), "59.94 rounds to 60");
        assert_eq!(inventory.formats[2].kind, MediaFormatKind::Audio);
        assert_eq!(inventory.formats[2].language.as_deref(), Some("de"));
        assert!(!inventory.truncated);
        assert!(inventory.has_muxed());
    }

    #[test]
    fn a_missing_vcodec_field_is_unknown_not_absent() {
        // Sites that report no codec at all still offer a real file; treating the absence
        // as "no video" would hide every format such a site has.
        let formats = vec![RawFormat {
            format_id: "plain".to_owned(),
            ext: Some("mp4".to_owned()),
            acodec: Some("aac".to_owned()),
            ..RawFormat::default()
        }];
        let inventory = normalize(&formats);
        // The audio codec is known, the video one is not — so the file may well carry
        // video, and yt-dlp's `b` counts it as a format with both. It used to come out as
        // `Audio`, contradicting the comment above (RD-120-50).
        assert_eq!(inventory.formats[0].kind, MediaFormatKind::Muxed);
        assert_eq!(inventory.formats[0].video_codec, None);
        assert_eq!(
            inventory.formats[0].audio_codec,
            Some(AudioCodecFamily::Aac)
        );
    }

    #[test]
    fn null_codecs_are_unknown_and_only_none_means_absent() {
        let formats = vec![
            // dumpert.nl: an HLS rendition with neither codec reported.
            RawFormat {
                format_id: "stream-3401".to_owned(),
                ext: Some("mp4".to_owned()),
                height: Some(1280),
                ..RawFormat::default()
            },
            // arte.tv: an HLS audio rendition, `vcodec: "none"` and `acodec: null`.
            RawFormat {
                format_id: "VOA-audio_0-Deutsch".to_owned(),
                vcodec: Some("none".to_owned()),
                language: Some("de".to_owned()),
                ..RawFormat::default()
            },
            // The same with the codec field present but empty.
            RawFormat {
                format_id: "video-only".to_owned(),
                vcodec: Some(" ".to_owned()),
                acodec: Some("none".to_owned()),
                ..RawFormat::default()
            },
        ];
        let inventory = normalize(&formats);
        let kinds: Vec<MediaFormatKind> = inventory.formats.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            vec![
                MediaFormatKind::Muxed,
                MediaFormatKind::Audio,
                MediaFormatKind::Video
            ]
        );
        assert!(inventory.formats.iter().all(|format| {
            format.video_codec.is_none()
                && format.audio_codec.is_none()
                && format.video_codec_raw.is_none()
                && format.audio_codec_raw.is_none()
        }));
    }

    #[test]
    fn picture_and_subtitle_pseudo_formats_are_dropped_whatever_their_codecs() {
        let pseudo = |id: &str, ext: &str| RawFormat {
            format_id: id.to_owned(),
            ext: Some(ext.to_owned()),
            ..RawFormat::default()
        };
        let formats = vec![
            pseudo("still", "jpg"),
            pseudo("thumb", "PNG"),
            pseudo("still-med-webp", "webp"),
            pseudo("thumbrail", "vtt"),
            pseudo("sb0", "mhtml"),
            pseudo("720p", "mp4"),
            pseudo("clip", "webm"),
        ];
        let inventory = normalize(&formats);
        let ids: Vec<&str> = inventory
            .formats
            .iter()
            .map(|format| format.format_id.as_str())
            .collect();
        assert_eq!(ids, vec!["720p", "clip"]);
    }

    #[test]
    fn oversized_inventories_are_marked_truncated() {
        let formats: Vec<RawFormat> = (0..MAX_MEDIA_FORMATS + 10)
            .map(|index| RawFormat {
                format_id: format!("f{index}"),
                vcodec: Some("avc1".to_owned()),
                acodec: Some("none".to_owned()),
                ..RawFormat::default()
            })
            .collect();
        let inventory = normalize(&formats);
        assert_eq!(inventory.formats.len(), MAX_MEDIA_FORMATS);
        assert!(inventory.truncated);
    }

    #[test]
    fn nonsense_bitrates_and_framerates_are_dropped() {
        let formats = vec![RawFormat {
            format_id: "odd".to_owned(),
            vcodec: Some("avc1".to_owned()),
            acodec: Some("none".to_owned()),
            tbr: Some(0.0),
            fps: Some(f64::NAN),
            ..RawFormat::default()
        }];
        let inventory = normalize(&formats);
        assert_eq!(inventory.formats[0].total_bitrate_kbps, None);
        assert_eq!(inventory.formats[0].fps, None);
    }
}
