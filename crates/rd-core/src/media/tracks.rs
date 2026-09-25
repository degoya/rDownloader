//! Audio tracks and subtitles: what a page offers and what should be done with it.
//!
//! Subtitles come from two places that must stay distinguishable. A *manual* track was
//! written by a person; an *automatic* one was produced by speech recognition and is
//! routinely wrong in ways that matter — names, numbers, negations. Burning an automatic
//! track into a file as though it were authored is a quiet lie, so the source travels with
//! every track and the two are filtered separately.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::criteria::{CriteriaError, MAX_CRITERIA_VALUES, is_criteria_token};

/// Most tracks kept from one probe, per kind.
pub const MAX_MEDIA_TRACKS: usize = 100;

/// Where a subtitle track came from.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleSource {
    /// Written by a person and published with the media.
    #[default]
    Manual,
    /// Produced by the site's speech recognition.
    Automatic,
}

/// One subtitle track a page offers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct SubtitleTrack {
    /// Language tag as reported, lowercased (`de`, `en-us`).
    pub language: String,
    /// The site's own name for the track (`English (auto-generated)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub source: SubtitleSource,
    /// Formats the track is available in (`vtt`, `srt`, `ttml`), lowercased.
    pub formats: Vec<String>,
}

/// One selectable audio track, derived from the format inventory.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct AudioTrack {
    /// Extractor format id of the underlying audio stream.
    pub format_id: String,
    /// Language tag as reported, lowercased; `None` when the site says nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codec: Option<super::format::AudioCodecFamily>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    /// The extractor's free-text note (`medium`, `original`), display only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What happens to a subtitle track that was asked for.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleMode {
    /// Nothing is fetched.
    #[default]
    Off,
    /// Written next to the media as its own file.
    Sidecar,
    /// Stored inside the container, when the container can hold subtitles.
    Embed,
    /// Both: a sidecar file *and* an embedded track.
    SidecarAndEmbed,
}

impl SubtitleMode {
    /// Whether a separate subtitle file is written.
    #[must_use]
    pub const fn writes_sidecar(self) -> bool {
        matches!(self, Self::Sidecar | Self::SidecarAndEmbed)
    }

    /// Whether the track is stored inside the container.
    #[must_use]
    pub const fn embeds(self) -> bool {
        matches!(self, Self::Embed | Self::SidecarAndEmbed)
    }

    /// Whether anything at all is fetched.
    #[must_use]
    pub const fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }
}

/// Which subtitles to fetch and what to do with them.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct SubtitlePolicy {
    pub mode: SubtitleMode,
    /// Languages to fetch; empty means every manual track the page offers.
    pub languages: Vec<String>,
    /// Whether speech-recognition tracks may be used when no manual one exists.
    ///
    /// Off by default: an automatic track is a guess, and a guess embedded in a file is
    /// indistinguishable from an authored translation once the file leaves here.
    pub include_automatic: bool,
    /// Sidecar format to convert to (`srt`, `vtt`, `ass`); `None` keeps what the site serves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convert_to: Option<String>,
}

/// Extra audio tracks to keep alongside the primary one.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct AudioTrackPolicy {
    /// Additional languages to merge in, beyond the one the format criteria picked.
    pub extra_languages: Vec<String>,
}

impl AudioTrackPolicy {
    /// Whether anything beyond the primary track was asked for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.extra_languages.is_empty()
    }
}

/// The track side of a media selection (RD-080-02).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct TrackSelection {
    pub audio: AudioTrackPolicy,
    pub subtitles: SubtitlePolicy,
}

impl TrackSelection {
    /// A selection that asks for nothing, borrowable without allocating.
    pub const EMPTY: Self = Self {
        audio: AudioTrackPolicy {
            extra_languages: Vec::new(),
        },
        subtitles: SubtitlePolicy {
            mode: SubtitleMode::Off,
            languages: Vec::new(),
            include_automatic: false,
            convert_to: None,
        },
    };

    /// Whether the selection asks for nothing beyond a plain download.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.audio.is_empty() && self.subtitles.mode.is_off()
    }

    /// Validates and normalises every free-form value.
    ///
    /// Language tags and subtitle formats reach a `--sub-langs` argument, so the same token
    /// rule as [`super::MediaFormatCriteria::sanitized`] applies: refuse, never escape.
    pub fn sanitized(mut self) -> Result<Self, CriteriaError> {
        self.audio.extra_languages = tokens(self.audio.extra_languages, "audio.extra_languages")?;
        self.subtitles.languages = tokens(self.subtitles.languages, "subtitles.languages")?;
        if let Some(format) = self.subtitles.convert_to.as_deref() {
            let format = format.trim().to_ascii_lowercase();
            if !is_criteria_token(&format) {
                return Err(CriteriaError::Token {
                    field: "subtitles.convert_to",
                });
            }
            self.subtitles.convert_to = Some(format);
        }
        Ok(self)
    }
}

fn tokens(values: Vec<String>, field: &'static str) -> Result<Vec<String>, CriteriaError> {
    if values.len() > MAX_CRITERIA_VALUES {
        return Err(CriteriaError::TooMany { field });
    }
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim().to_ascii_lowercase();
        if !is_criteria_token(&value) {
            return Err(CriteriaError::Token { field });
        }
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

/// Why a track selection cannot be honoured as asked.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TrackWarning {
    /// The container cannot hold more than one audio track.
    MultipleAudioUnsupported { container: String },
    /// The container cannot hold embedded subtitles.
    SubtitleEmbedUnsupported { container: String },
    /// A requested language is not offered by the page.
    LanguageUnavailable { language: String },
    /// Only an automatic track exists for a requested language.
    OnlyAutomaticAvailable { language: String },
    /// ffmpeg is needed to merge or embed and is not available.
    ToolUnavailable,
}

/// Checks a track selection against what the page and the container allow.
///
/// Returns warnings rather than refusing: a missing subtitle language should not stop a
/// download that is otherwise exactly what was asked for.
#[must_use]
pub fn track_warnings(
    selection: &TrackSelection,
    container: &str,
    audio: &[AudioTrack],
    subtitles: &[SubtitleTrack],
    can_merge: bool,
) -> Vec<TrackWarning> {
    let mut warnings = Vec::new();
    if !selection.audio.is_empty() && !super::container::supports_multiple_audio(container) {
        warnings.push(TrackWarning::MultipleAudioUnsupported {
            container: container.to_owned(),
        });
    }
    if selection.subtitles.mode.embeds() && !super::container::supports_subtitles(container) {
        warnings.push(TrackWarning::SubtitleEmbedUnsupported {
            container: container.to_owned(),
        });
    }
    if !can_merge && (!selection.audio.is_empty() || selection.subtitles.mode.embeds()) {
        warnings.push(TrackWarning::ToolUnavailable);
    }
    for language in &selection.audio.extra_languages {
        if !audio.iter().any(|track| {
            track
                .language
                .as_deref()
                .is_some_and(|value| matches(value, language))
        }) {
            warnings.push(TrackWarning::LanguageUnavailable {
                language: language.clone(),
            });
        }
    }
    for language in &selection.subtitles.languages {
        let available: Vec<&SubtitleTrack> = subtitles
            .iter()
            .filter(|track| matches(&track.language, language))
            .collect();
        if available.is_empty() {
            warnings.push(TrackWarning::LanguageUnavailable {
                language: language.clone(),
            });
        } else if available
            .iter()
            .all(|track| track.source == SubtitleSource::Automatic)
            && !selection.subtitles.include_automatic
        {
            warnings.push(TrackWarning::OnlyAutomaticAvailable {
                language: language.clone(),
            });
        }
    }
    warnings
}

/// `de` matches `de-DE`; `de-DE` does not match `de-CH`.
fn matches(language: &str, wanted: &str) -> bool {
    language == wanted
        || language
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::{
        AudioTrack, AudioTrackPolicy, SubtitleMode, SubtitlePolicy, SubtitleSource, SubtitleTrack,
        TrackSelection, TrackWarning, track_warnings,
    };

    fn subtitle(language: &str, source: SubtitleSource) -> SubtitleTrack {
        SubtitleTrack {
            language: language.to_owned(),
            name: None,
            source,
            formats: vec!["vtt".to_owned()],
        }
    }

    fn audio(language: &str) -> AudioTrack {
        AudioTrack {
            format_id: format!("a-{language}"),
            language: Some(language.to_owned()),
            ..AudioTrack::default()
        }
    }

    #[test]
    fn an_mp4_holds_several_audio_tracks_but_an_mp3_does_not() {
        let selection = TrackSelection {
            audio: AudioTrackPolicy {
                extra_languages: vec!["de".to_owned()],
            },
            ..TrackSelection::default()
        };
        let tracks = [audio("de"), audio("en")];
        assert!(track_warnings(&selection, "mp4", &tracks, &[], true).is_empty());
        assert_eq!(
            track_warnings(&selection, "mp3", &tracks, &[], true),
            vec![TrackWarning::MultipleAudioUnsupported {
                container: "mp3".to_owned()
            }]
        );
    }

    #[test]
    fn an_automatic_track_is_not_silently_used_in_place_of_a_manual_one() {
        let selection = TrackSelection {
            subtitles: SubtitlePolicy {
                mode: SubtitleMode::Embed,
                languages: vec!["de".to_owned()],
                include_automatic: false,
                convert_to: None,
            },
            ..TrackSelection::default()
        };
        let only_auto = [subtitle("de", SubtitleSource::Automatic)];
        assert_eq!(
            track_warnings(&selection, "mkv", &[], &only_auto, true),
            vec![TrackWarning::OnlyAutomaticAvailable {
                language: "de".to_owned()
            }]
        );

        // With the opt-in, the automatic track is acceptable and nothing is reported.
        let opted_in = TrackSelection {
            subtitles: SubtitlePolicy {
                include_automatic: true,
                ..selection.subtitles.clone()
            },
            ..selection.clone()
        };
        assert!(track_warnings(&opted_in, "mkv", &[], &only_auto, true).is_empty());

        // A manual track is used without any opt-in.
        let manual = [subtitle("de", SubtitleSource::Manual)];
        assert!(track_warnings(&selection, "mkv", &[], &manual, true).is_empty());
    }

    #[test]
    fn a_missing_language_is_reported_but_does_not_refuse_the_selection() {
        let selection = TrackSelection {
            subtitles: SubtitlePolicy {
                mode: SubtitleMode::Sidecar,
                languages: vec!["fi".to_owned()],
                ..SubtitlePolicy::default()
            },
            ..TrackSelection::default()
        };
        assert_eq!(
            track_warnings(
                &selection,
                "mp4",
                &[],
                &[subtitle("de", SubtitleSource::Manual)],
                true
            ),
            vec![TrackWarning::LanguageUnavailable {
                language: "fi".to_owned()
            }]
        );
    }

    #[test]
    fn embedding_without_ffmpeg_is_flagged() {
        let selection = TrackSelection {
            subtitles: SubtitlePolicy {
                mode: SubtitleMode::Embed,
                ..SubtitlePolicy::default()
            },
            ..TrackSelection::default()
        };
        assert!(
            track_warnings(&selection, "mkv", &[], &[], false)
                .contains(&TrackWarning::ToolUnavailable)
        );
        // A sidecar needs no merging, so it stays available.
        let sidecar = TrackSelection {
            subtitles: SubtitlePolicy {
                mode: SubtitleMode::Sidecar,
                ..SubtitlePolicy::default()
            },
            ..TrackSelection::default()
        };
        assert!(track_warnings(&sidecar, "mkv", &[], &[], false).is_empty());
    }

    #[test]
    fn language_tokens_are_normalised_and_injection_is_refused() {
        let selection = TrackSelection {
            subtitles: SubtitlePolicy {
                languages: vec!["DE".to_owned(), "en-US".to_owned(), "de".to_owned()],
                ..SubtitlePolicy::default()
            },
            ..TrackSelection::default()
        }
        .sanitized()
        .expect("plain tokens");
        assert_eq!(selection.subtitles.languages, vec!["de", "en-us"]);

        let bad = TrackSelection {
            subtitles: SubtitlePolicy {
                languages: vec!["de,all".to_owned()],
                ..SubtitlePolicy::default()
            },
            ..TrackSelection::default()
        };
        assert!(bad.sanitized().is_err(), "a comma would widen --sub-langs");
    }

    #[test]
    fn subtitle_modes_say_what_they_produce() {
        assert!(SubtitleMode::Off.is_off());
        assert!(SubtitleMode::Sidecar.writes_sidecar());
        assert!(!SubtitleMode::Sidecar.embeds());
        assert!(SubtitleMode::Embed.embeds());
        assert!(!SubtitleMode::Embed.writes_sidecar());
        assert!(SubtitleMode::SidecarAndEmbed.embeds());
        assert!(SubtitleMode::SidecarAndEmbed.writes_sidecar());
    }
}
