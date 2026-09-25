//! What a media link was chosen *by*, rather than which format id happened to satisfy it.
//!
//! A `format_id` is the site's, not ours: YouTube rotates them, and a link that sat in the
//! queue overnight resolves to a different one in the morning. Storing the criteria and
//! re-resolving at download time is the only way "1080p AV1 without HDR" still means that
//! tomorrow. The legacy presets (`best`, `1080p`, `audio_mp3`) are expressed in the same
//! vocabulary by [`MediaFormatCriteria::preset`], so old rows and old API callers keep
//! working without a data migration.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::embed::MediaEmbedPolicy;
use super::format::{AudioCodecFamily, DynamicRange, VideoCodecFamily};
use super::tracks::TrackSelection;

/// Longest free-form token (container, codec name, language tag) accepted.
pub const MAX_CRITERIA_TOKEN: usize = 32;
/// Most entries accepted in any one criteria list.
pub const MAX_CRITERIA_VALUES: usize = 32;

/// Whether the job wants a video file or audio only.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaTarget {
    #[default]
    Video,
    AudioOnly,
}

/// What happens to the streams once they are fetched.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum MediaOutput {
    /// Keep whatever the chosen format(s) produce.
    #[default]
    Passthrough,
    /// Merge/remux into a fixed container without re-encoding.
    Remux { container: String },
    /// Extract the audio track into `codec` (`mp3`, `m4a`, `opus`, `flac`).
    ExtractAudio {
        codec: String,
        /// yt-dlp audio quality, `0` (best) to `9`.
        #[serde(default)]
        quality: u8,
    },
}

/// How hard a filter is.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaStrictness {
    /// Drop criteria in a fixed order until something matches, and record what was dropped.
    #[default]
    Preferred,
    /// Fail loudly rather than quietly hand over something the user did not ask for.
    Required,
}

/// Why [`MediaFormatCriteria::sanitized`] refused a value.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CriteriaError {
    #[error("`{field}` contains a value that is not a plain token")]
    Token { field: &'static str },
    #[error("`{field}` holds more than {MAX_CRITERIA_VALUES} values")]
    TooMany { field: &'static str },
    #[error("`{field}` has a minimum above its maximum")]
    Range { field: &'static str },
    #[error("`{field}` is out of range")]
    Value { field: &'static str },
}

/// The persisted selection. Every field is optional or empty-as-any, so an older blob
/// deserialises into "no opinion" rather than into a filter nobody asked for.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct MediaFormatCriteria {
    pub target: MediaTarget,
    /// Acceptable containers; empty means any.
    pub containers: Vec<String>,
    pub video_codecs: Vec<VideoCodecFamily>,
    pub audio_codecs: Vec<AudioCodecFamily>,
    pub dynamic_range: Vec<DynamicRange>,
    pub min_height: Option<u32>,
    pub max_height: Option<u32>,
    pub min_fps: Option<u32>,
    pub max_fps: Option<u32>,
    pub max_total_bitrate_kbps: Option<u32>,
    pub min_audio_bitrate_kbps: Option<u32>,
    /// Preferred audio languages, best first. A list rather than a single value because
    /// multiple audio tracks (RD-080-02) extend exactly this field.
    pub audio_languages: Vec<String>,
    pub output: MediaOutput,
    /// Whether a separate video and audio stream may be merged. Forced off when ffmpeg is
    /// missing, which is what leaves a muxed-only fallback.
    pub allow_merge: bool,
    pub strictness: MediaStrictness,
    /// Extra audio tracks and subtitles (RD-080-02). Empty for a plain download.
    ///
    /// Always serialised, unlike the optional fields around it: the generated client type
    /// declares it required, and omitting it would hand the UI an `undefined` to trip over.
    #[serde(default)]
    pub tracks: TrackSelection,
    /// What to write into the finished file and what to cut out of it (RD-080-03).
    /// Always serialised, for the same reason as `tracks`.
    #[serde(default)]
    pub embed: MediaEmbedPolicy,
    /// Output template for this job (RD-080-05); `None` uses the configured default.
    ///
    /// Neither validated nor expanded here: the evaluator lives in `rd-files`, beside the
    /// sanitiser it depends on, and `rd-files` depends on this crate. The API layer runs
    /// `rd_files::validate` before storing, and the runner falls back to the plain file name
    /// if a stored template turns out not to expand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_template: Option<String>,
    /// The preset these criteria came from (`best`, `1080p`, `audio_mp3`), or `None` for a
    /// hand-built selection. Display only — the criteria are the contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

/// Preset ids that predate the selector and must keep resolving.
pub const LEGACY_PRESETS: [&str; 7] = [
    "best",
    "2160p",
    "1440p",
    "1080p",
    "720p",
    "480p",
    "audio_mp3",
];

impl MediaFormatCriteria {
    /// The criteria a legacy variant id stands for, or `None` when the id is not a preset.
    ///
    /// This is the bridge that keeps `downloads.media_json` rows written before RD-080-01
    /// — and every API caller still sending `media_variant: "1080p"` — working unchanged.
    #[must_use]
    pub fn preset(id: &str) -> Option<Self> {
        let base = Self {
            allow_merge: true,
            preset: Some(id.to_owned()),
            ..Self::default()
        };
        match id {
            "best" => Some(Self {
                output: MediaOutput::Remux {
                    container: "mp4".to_owned(),
                },
                ..base
            }),
            "audio_mp3" => Some(Self {
                target: MediaTarget::AudioOnly,
                output: MediaOutput::ExtractAudio {
                    codec: "mp3".to_owned(),
                    quality: 0,
                },
                ..base
            }),
            _ => {
                let height = id.strip_suffix('p')?.parse::<u32>().ok()?;
                Some(Self {
                    max_height: Some(height),
                    output: MediaOutput::Remux {
                        container: "mp4".to_owned(),
                    },
                    ..base
                })
            }
        }
    }

    /// Criteria for the default preset, falling back to `best` for an unknown id.
    #[must_use]
    pub fn preset_or_best(id: &str) -> Self {
        Self::preset(id).unwrap_or_else(|| {
            Self::preset("best").unwrap_or_else(|| Self {
                allow_merge: true,
                ..Self::default()
            })
        })
    }

    /// Validates every free-form value and normalises it to lowercase.
    ///
    /// This is a security boundary, not tidiness: containers, codec names and language tags
    /// are interpolated into a yt-dlp `-f` expression, where `[`, `]` and `,` are syntax. A
    /// value like `mp4][url*=evil` must be refused here rather than escaped later, because
    /// there is exactly one place that can be relied on to do it — this one.
    pub fn sanitized(mut self) -> Result<Self, CriteriaError> {
        self.containers = normalize_tokens(self.containers, "containers")?;
        self.audio_languages = normalize_tokens(self.audio_languages, "audio_languages")?;
        if self.video_codecs.len() > MAX_CRITERIA_VALUES {
            return Err(CriteriaError::TooMany {
                field: "video_codecs",
            });
        }
        if self.audio_codecs.len() > MAX_CRITERIA_VALUES {
            return Err(CriteriaError::TooMany {
                field: "audio_codecs",
            });
        }
        if self.dynamic_range.len() > MAX_CRITERIA_VALUES {
            return Err(CriteriaError::TooMany {
                field: "dynamic_range",
            });
        }
        check_range(self.min_height, self.max_height, "height")?;
        check_range(self.min_fps, self.max_fps, "fps")?;
        match &mut self.output {
            MediaOutput::Passthrough => {}
            MediaOutput::Remux { container } => {
                *container = normalize_token(container, "output.container")?;
            }
            MediaOutput::ExtractAudio { codec, quality } => {
                *codec = normalize_token(codec, "output.codec")?;
                if *quality > 9 {
                    return Err(CriteriaError::Value {
                        field: "output.quality",
                    });
                }
            }
        }
        self.tracks = self.tracks.sanitized()?;
        self.embed = self.embed.sanitized()?;
        if let Some(preset) = &self.preset {
            let preset = normalize_token(preset, "preset")?;
            if !LEGACY_PRESETS.contains(&preset.as_str()) && preset != "custom" {
                return Err(CriteriaError::Value { field: "preset" });
            }
            self.preset = Some(preset);
        }
        Ok(self)
    }

    /// Whether any filter at all is set. Used to tell "no opinion" apart from "everything
    /// was filtered away", which are very different things to report to a user.
    #[must_use]
    pub fn is_unfiltered(&self) -> bool {
        self.containers.is_empty()
            && self.video_codecs.is_empty()
            && self.audio_codecs.is_empty()
            && self.dynamic_range.is_empty()
            && self.min_height.is_none()
            && self.max_height.is_none()
            && self.min_fps.is_none()
            && self.max_fps.is_none()
            && self.max_total_bitrate_kbps.is_none()
            && self.min_audio_bitrate_kbps.is_none()
            && self.audio_languages.is_empty()
    }

    /// The container the finished file will have, when the output pins one.
    #[must_use]
    pub fn output_container(&self) -> Option<&str> {
        match &self.output {
            MediaOutput::Passthrough => None,
            MediaOutput::Remux { container } => Some(container),
            MediaOutput::ExtractAudio { codec, .. } => Some(codec),
        }
    }
}

/// `true` for a token of `[a-z0-9._-]{1,MAX_CRITERIA_TOKEN}`.
#[must_use]
pub fn is_criteria_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CRITERIA_TOKEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn normalize_token(value: &str, field: &'static str) -> Result<String, CriteriaError> {
    let value = value.trim().trim_start_matches('.').to_ascii_lowercase();
    if is_criteria_token(&value) {
        Ok(value)
    } else {
        Err(CriteriaError::Token { field })
    }
}

fn normalize_tokens(
    values: Vec<String>,
    field: &'static str,
) -> Result<Vec<String>, CriteriaError> {
    if values.len() > MAX_CRITERIA_VALUES {
        return Err(CriteriaError::TooMany { field });
    }
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let value = normalize_token(&value, field)?;
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

fn check_range(
    min: Option<u32>,
    max: Option<u32>,
    field: &'static str,
) -> Result<(), CriteriaError> {
    match (min, max) {
        (Some(min), Some(max)) if min > max => Err(CriteriaError::Range { field }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CriteriaError, LEGACY_PRESETS, MediaFormatCriteria, MediaOutput, MediaTarget};

    #[test]
    fn every_legacy_preset_round_trips() {
        for id in LEGACY_PRESETS {
            let criteria = MediaFormatCriteria::preset(id)
                .unwrap_or_else(|| panic!("{id} is not a known preset"));
            assert_eq!(criteria.preset.as_deref(), Some(id));
            let sanitized = criteria.clone().sanitized().expect("preset sanitizes");
            assert_eq!(sanitized, criteria);
        }
        assert_eq!(
            MediaFormatCriteria::preset("1080p").and_then(|c| c.max_height),
            Some(1080)
        );
        assert_eq!(
            MediaFormatCriteria::preset("audio_mp3").map(|c| c.target),
            Some(MediaTarget::AudioOnly)
        );
        assert_eq!(MediaFormatCriteria::preset("8k"), None);
        assert_eq!(MediaFormatCriteria::preset("720"), None);
    }

    #[test]
    fn unknown_presets_fall_back_to_best() {
        let criteria = MediaFormatCriteria::preset_or_best("nonsense");
        assert_eq!(criteria.preset.as_deref(), Some("best"));
        assert_eq!(criteria.max_height, None);
    }

    #[test]
    fn injection_shaped_tokens_are_refused_not_escaped() {
        let criteria = MediaFormatCriteria {
            containers: vec!["mp4][url*=evil".to_owned()],
            ..MediaFormatCriteria::default()
        };
        assert_eq!(
            criteria.sanitized(),
            Err(CriteriaError::Token {
                field: "containers"
            })
        );

        for bad in [
            "mp4]",
            "mp 4",
            "mp4,webm",
            "mp4[height<=1]",
            "",
            "a".repeat(33).as_str(),
        ] {
            let criteria = MediaFormatCriteria {
                containers: vec![bad.to_owned()],
                ..MediaFormatCriteria::default()
            };
            assert!(
                criteria.sanitized().is_err(),
                "{bad:?} should have been refused"
            );
        }
    }

    #[test]
    fn tokens_are_lowercased_and_deduplicated() {
        let criteria = MediaFormatCriteria {
            containers: vec!["MP4".to_owned(), ".mp4".to_owned(), "WebM".to_owned()],
            audio_languages: vec!["DE".to_owned(), "en-US".to_owned()],
            ..MediaFormatCriteria::default()
        }
        .sanitized()
        .expect("plain tokens");
        assert_eq!(criteria.containers, vec!["mp4", "webm"]);
        assert_eq!(criteria.audio_languages, vec!["de", "en-us"]);
    }

    #[test]
    fn inverted_ranges_and_bad_quality_are_refused() {
        let criteria = MediaFormatCriteria {
            min_height: Some(1080),
            max_height: Some(720),
            ..MediaFormatCriteria::default()
        };
        assert_eq!(
            criteria.sanitized(),
            Err(CriteriaError::Range { field: "height" })
        );

        let criteria = MediaFormatCriteria {
            output: MediaOutput::ExtractAudio {
                codec: "mp3".to_owned(),
                quality: 42,
            },
            ..MediaFormatCriteria::default()
        };
        assert!(criteria.sanitized().is_err());
    }

    #[test]
    fn an_empty_criteria_set_is_unfiltered() {
        assert!(MediaFormatCriteria::default().is_unfiltered());
        assert!(
            MediaFormatCriteria::preset("best")
                .expect("best")
                .is_unfiltered()
        );
        assert!(
            !MediaFormatCriteria::preset("1080p")
                .expect("1080p")
                .is_unfiltered()
        );
    }
}
