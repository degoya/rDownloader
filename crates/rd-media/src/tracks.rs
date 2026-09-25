//! Derives the selectable audio and subtitle tracks from what yt-dlp reported.
//!
//! Audio tracks are not a separate list in yt-dlp's output — they are the audio-only entries
//! of `formats[]`, one per language and bitrate. Subtitles *are* separate, and come in two
//! maps: `subtitles` for authored tracks and `automatic_captions` for recognised ones. The
//! two are merged into one list here, with the source kept on every entry, because that
//! distinction is the whole reason a person cares which track they get.

use std::collections::BTreeMap;

use rd_core::{
    AudioTrack, MAX_MEDIA_TRACKS, MediaFormatInventory, MediaFormatKind, SubtitleSource,
    SubtitleTrack,
};
use serde::Deserialize;

/// One entry of a yt-dlp `subtitles` / `automatic_captions` map.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RawSubtitle {
    pub ext: Option<String>,
    pub name: Option<String>,
}

/// The subtitle maps of one probe.
pub type RawSubtitleMap = BTreeMap<String, Vec<RawSubtitle>>;

/// The audio-only formats of an inventory, as selectable tracks.
///
/// Kept in the inventory's order so the list is stable across probes, and capped like every
/// other list that comes from a site.
#[must_use]
pub fn audio_tracks(inventory: &MediaFormatInventory) -> Vec<AudioTrack> {
    inventory
        .of_kind(MediaFormatKind::Audio)
        .take(MAX_MEDIA_TRACKS)
        .map(|format| AudioTrack {
            format_id: format.format_id.clone(),
            language: format.language.clone(),
            codec: format.audio_codec,
            bitrate_kbps: format.audio_bitrate_kbps,
            note: format.format_note.clone(),
        })
        .collect()
}

/// Merges the manual and automatic subtitle maps into one list.
///
/// A language present in both keeps its manual entry *and* its automatic one, rather than
/// the manual one shadowing the automatic: someone who explicitly opted into automatic
/// captions may well be after the auto-translated variant a site only offers there.
#[must_use]
pub fn subtitle_tracks(manual: &RawSubtitleMap, automatic: &RawSubtitleMap) -> Vec<SubtitleTrack> {
    let mut tracks = Vec::new();
    for (source, map) in [
        (SubtitleSource::Manual, manual),
        (SubtitleSource::Automatic, automatic),
    ] {
        for (language, entries) in map {
            if tracks.len() >= MAX_MEDIA_TRACKS {
                return tracks;
            }
            let language = language.trim().to_ascii_lowercase();
            if language.is_empty() {
                continue;
            }
            let mut formats: Vec<String> = entries
                .iter()
                .filter_map(|entry| entry.ext.as_deref())
                .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
                .collect();
            formats.sort();
            formats.dedup();
            tracks.push(SubtitleTrack {
                language,
                name: entries.iter().find_map(|entry| entry.name.clone()),
                source,
                formats,
            });
        }
    }
    tracks
}

/// The `--sub-langs` value for a policy, or `None` when nothing should be fetched.
///
/// Every token has already passed [`rd_core::TrackSelection::sanitized`], so the join is a
/// join and not an escaping problem.
#[must_use]
pub fn sub_langs(policy: &rd_core::SubtitlePolicy) -> Option<String> {
    if policy.mode.is_off() {
        return None;
    }
    if policy.languages.is_empty() {
        return Some("all".to_owned());
    }
    Some(policy.languages.join(","))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rd_core::{SubtitleMode, SubtitlePolicy, SubtitleSource};

    use super::{RawSubtitle, RawSubtitleMap, sub_langs, subtitle_tracks};

    fn map(entries: &[(&str, &str)]) -> RawSubtitleMap {
        let mut map = BTreeMap::new();
        for (language, ext) in entries {
            map.insert(
                (*language).to_owned(),
                vec![RawSubtitle {
                    ext: Some((*ext).to_owned()),
                    name: None,
                }],
            );
        }
        map
    }

    #[test]
    fn manual_and_automatic_tracks_stay_distinguishable() {
        let tracks = subtitle_tracks(
            &map(&[("en", "vtt"), ("DE", "srt")]),
            &map(&[("en", "vtt")]),
        );
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0].language, "de", "keys are lowercased");
        assert_eq!(tracks[0].source, SubtitleSource::Manual);
        assert_eq!(tracks[1].language, "en");
        assert_eq!(tracks[1].source, SubtitleSource::Manual);
        assert_eq!(tracks[2].language, "en");
        assert_eq!(
            tracks[2].source,
            SubtitleSource::Automatic,
            "the automatic entry survives alongside the manual one"
        );
    }

    #[test]
    fn an_off_policy_asks_for_nothing() {
        assert_eq!(sub_langs(&SubtitlePolicy::default()), None);
        assert_eq!(
            sub_langs(&SubtitlePolicy {
                mode: SubtitleMode::Sidecar,
                ..SubtitlePolicy::default()
            }),
            Some("all".to_owned()),
            "no language filter means every track the page offers"
        );
        assert_eq!(
            sub_langs(&SubtitlePolicy {
                mode: SubtitleMode::Embed,
                languages: vec!["de".to_owned(), "en".to_owned()],
                ..SubtitlePolicy::default()
            }),
            Some("de,en".to_owned())
        );
    }
}
