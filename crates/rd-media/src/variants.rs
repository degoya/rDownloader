//! Turns the extractor's raw format list into the short, presentable list shown next to a
//! link.
//!
//! The presets (`best`, `1080p`, …, `audio_mp3`) are no longer special-cased expressions:
//! each one is a [`MediaFormatCriteria`] resolved against the normalised inventory, so a
//! preset and a hand-built selection go through exactly the same code. What people see in
//! the dropdown and what the runner downloads therefore cannot drift apart.

use rd_core::{MediaFormatCriteria, MediaFormatInventory, MediaKind, MediaTarget, MediaVariant};

use crate::{
    format_inventory::{RawFormat, normalize},
    select::{MediaCapabilities, resolve},
};

/// Resolutions offered as presets, richest first.
const HEIGHTS: [u32; 5] = [2160, 1440, 1080, 720, 480];

/// `best`, one entry per available height (≤ 2160p), and `audio_mp3`, assuming a complete
/// ffmpeg. Returns the variants plus the id that should be selected for `preferred`.
#[must_use]
pub fn synthesize_variants(formats: &[RawFormat], preferred: &str) -> (Vec<MediaVariant>, String) {
    synthesize_from_inventory(
        &normalize(formats),
        preferred,
        MediaCapabilities::complete(),
    )
}

/// The preset variants a page offers, honouring what the installed tools can do.
///
/// With no ffmpeg, `capabilities` leaves only progressive formats reachable, so a page that
/// serves nothing but separate streams yields no video presets at all rather than presets
/// that would fail at download time.
#[must_use]
pub fn synthesize_from_inventory(
    inventory: &MediaFormatInventory,
    preferred: &str,
    capabilities: MediaCapabilities,
) -> (Vec<MediaVariant>, String) {
    let mut variants = Vec::new();
    if let Some(variant) = preset_variant(inventory, "best", "Video (best)", capabilities) {
        variants.push(variant);
    }
    for height in HEIGHTS {
        if !offers_height(inventory, height) {
            continue;
        }
        let id = format!("{height}p");
        let label = format!("Video {height}p");
        if let Some(mut variant) = preset_variant(inventory, &id, &label, capabilities) {
            // The label promises a resolution ceiling, so report that rather than whatever
            // the page happened to have just below it.
            variant.height = Some(height);
            variants.push(variant);
        }
    }
    if let Some(variant) = preset_variant(inventory, "audio_mp3", "Audio (MP3)", capabilities) {
        variants.push(variant);
    }
    if variants.is_empty() {
        variants = fallback_variants(inventory, capabilities);
    }
    let selected = if variants.iter().any(|variant| variant.id == preferred) {
        preferred.to_owned()
    } else {
        "best".to_owned()
    };
    (variants, selected)
}

/// Whether offering a `{height}p` preset is honest: the page must have something at or
/// above that height (otherwise the preset is a duplicate of a lower one) and something at
/// or below it (otherwise there is nothing to download).
fn offers_height(inventory: &MediaFormatInventory, height: u32) -> bool {
    let video: Vec<&rd_core::MediaFormat> = inventory
        .formats
        .iter()
        .filter(|format| format.has_video())
        .collect();
    video
        .iter()
        .any(|format| format.height.is_some_and(|value| value >= height))
        && video
            .iter()
            .any(|format| format.height.is_some_and(|value| value <= height))
}

/// What is offered when not one preset resolves (RD-120-50).
///
/// Until `a2beadf6` a page always offered `best` as `bv*+ba/b` and let yt-dlp choose; the
/// criteria rewrite dropped that, so a page whose inventory resolves nothing — a flat
/// playlist entry that has no formats yet, a site whose formats defeat the normaliser, a
/// page of video streams without audio — came out with no variant and was queued without a
/// selection. The fallback hands the choice back to yt-dlp, whose own `b` falls back to an
/// incomplete format where a page has nothing else.
///
/// What `a2beadf6` fixed stays fixed: without ffmpeg, a page *known* to serve only separate
/// streams still offers nothing, because every expression would need a merge. Only an empty
/// inventory — nothing is known either way — gets the merge-free `b`.
fn fallback_variants(
    inventory: &MediaFormatInventory,
    capabilities: MediaCapabilities,
) -> Vec<MediaVariant> {
    let format = if capabilities.can_merge {
        "bv*+ba/b"
    } else if inventory.is_empty() {
        "b"
    } else {
        return Vec::new();
    };
    let criteria = MediaFormatCriteria {
        allow_merge: capabilities.can_merge,
        ..MediaFormatCriteria::preset_or_best("best")
    };
    let mut variants = vec![MediaVariant {
        id: "best".to_owned(),
        label: "Video (best)".to_owned(),
        kind: MediaKind::Video,
        ext: criteria.output_container().unwrap_or("mp4").to_owned(),
        height: None,
        abr: None,
        filesize_approx: None,
        format: format.to_owned(),
        fps: None,
        dynamic_range: rd_core::DynamicRange::Unknown,
        video_codec: None,
        audio_codec: None,
        requires_merge: false,
        warnings: Vec::new(),
        criteria: Some(criteria),
    }];
    // An empty inventory says nothing against audio either, and a playlist of songs is the
    // case the MP3 preset exists for. Extraction needs ffmpeg, as for the resolved preset.
    if inventory.is_empty()
        && capabilities.can_transcode_audio
        && let Some(preset) = MediaFormatCriteria::preset("audio_mp3")
    {
        variants.push(MediaVariant {
            id: "audio_mp3".to_owned(),
            label: "Audio (MP3)".to_owned(),
            kind: MediaKind::Audio,
            ext: "mp3".to_owned(),
            height: None,
            abr: None,
            filesize_approx: None,
            format: "ba/b".to_owned(),
            fps: None,
            dynamic_range: rd_core::DynamicRange::Unknown,
            video_codec: None,
            audio_codec: None,
            requires_merge: false,
            warnings: Vec::new(),
            criteria: Some(preset),
        });
    }
    variants
}

/// One preset resolved against the inventory, or `None` when it cannot be satisfied.
fn preset_variant(
    inventory: &MediaFormatInventory,
    id: &str,
    label: &str,
    capabilities: MediaCapabilities,
) -> Option<MediaVariant> {
    let criteria = MediaFormatCriteria {
        allow_merge: capabilities.can_merge,
        ..MediaFormatCriteria::preset(id)?
    };
    // Audio extraction needs ffmpeg; offering MP3 without it would fail at download time.
    if criteria.target == MediaTarget::AudioOnly && !capabilities.can_transcode_audio {
        return None;
    }
    let resolution = resolve(inventory, &criteria, capabilities).ok()?;
    let primary = resolution.video.as_ref()?;
    let kind = if criteria.target == MediaTarget::AudioOnly {
        MediaKind::Audio
    } else {
        MediaKind::Video
    };
    Some(MediaVariant {
        id: id.to_owned(),
        label: label.to_owned(),
        kind,
        ext: resolution.container.clone(),
        height: primary.height,
        abr: resolution
            .audio
            .as_ref()
            .and_then(|format| format.audio_bitrate_kbps)
            .or(primary.audio_bitrate_kbps),
        filesize_approx: resolution.estimated_bytes,
        format: resolution.format_expression.clone(),
        fps: primary.fps,
        dynamic_range: primary.dynamic_range,
        video_codec: primary.video_codec,
        audio_codec: resolution
            .audio
            .as_ref()
            .and_then(|format| format.audio_codec)
            .or(primary.audio_codec),
        requires_merge: resolution.audio.is_some(),
        warnings: resolution.warnings.clone(),
        criteria: Some(criteria),
    })
}

#[cfg(test)]
mod tests {
    use crate::{format_inventory::normalize, select::MediaCapabilities};

    use super::{RawFormat, synthesize_from_inventory, synthesize_variants};

    fn video(height: u32, size: u64) -> RawFormat {
        RawFormat {
            format_id: format!("v{height}"),
            height: Some(height),
            vcodec: Some("avc1".to_owned()),
            acodec: Some("none".to_owned()),
            ext: Some("mp4".to_owned()),
            filesize: Some(size),
            ..RawFormat::default()
        }
    }

    fn audio() -> RawFormat {
        RawFormat {
            format_id: "a1".to_owned(),
            vcodec: Some("none".to_owned()),
            acodec: Some("opus".to_owned()),
            abr: Some(128.0),
            filesize: Some(50),
            ..RawFormat::default()
        }
    }

    #[test]
    fn offers_only_available_heights_and_audio() {
        let formats = vec![video(1080, 900), video(720, 500), video(360, 100), audio()];
        let (variants, selected) = synthesize_variants(&formats, "720p");
        let ids: Vec<&str> = variants.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, vec!["best", "1080p", "720p", "480p", "audio_mp3"]);
        assert_eq!(selected, "720p");
        assert_eq!(variants[0].height, Some(1080));
        assert_eq!(variants.last().expect("audio").abr, Some(128));
        let (_, fallback) = synthesize_variants(&formats, "8k");
        assert_eq!(fallback, "best");
    }

    #[test]
    fn video_presets_carry_the_merge_flag_and_a_pinned_expression() {
        let formats = vec![video(1080, 900), audio()];
        let (variants, _) = synthesize_variants(&formats, "best");
        let best = variants.first().expect("best");
        assert!(best.requires_merge, "1080p video needs the audio merged on");
        assert!(
            best.format.starts_with("v1080+a1/"),
            "the pinned ids come first: {}",
            best.format
        );
        assert!(
            best.format.ends_with("/b"),
            "the last alternative is merge-free: {}",
            best.format
        );
        assert_eq!(best.ext, "mp4");
    }

    #[test]
    fn without_ffmpeg_only_progressive_presets_remain() {
        let progressive = RawFormat {
            format_id: "p720".to_owned(),
            height: Some(720),
            vcodec: Some("avc1".to_owned()),
            acodec: Some("mp4a.40.2".to_owned()),
            ext: Some("mp4".to_owned()),
            filesize: Some(400),
            ..RawFormat::default()
        };
        let inventory = normalize(&[video(1080, 900), progressive, audio()]);
        let (variants, _) =
            synthesize_from_inventory(&inventory, "best", MediaCapabilities::ytdlp_only());
        let ids: Vec<&str> = variants.iter().map(|v| v.id.as_str()).collect();
        assert!(!ids.contains(&"audio_mp3"), "MP3 needs ffmpeg: {ids:?}");
        assert!(
            variants.iter().all(|variant| !variant.requires_merge),
            "nothing may require a merge: {ids:?}"
        );
        assert!(
            variants.iter().all(|variant| !variant.format.contains('+')),
            "no expression may ask for a merge: {ids:?}"
        );
        let best = variants.first().expect("best is still offered");
        assert_eq!(best.height, Some(720), "the progressive stream, not 1080p");
    }

    #[test]
    fn a_page_that_resolves_no_preset_still_offers_best() {
        // Video streams and not one audio stream: no preset resolves, yet yt-dlp's own `b`
        // downloads the video. This is the fallback `a2beadf6` removed (RD-120-50).
        let inventory = normalize(&[video(1080, 900), video(720, 500)]);
        let (variants, selected) =
            synthesize_from_inventory(&inventory, "720p", MediaCapabilities::complete());
        let ids: Vec<&str> = variants.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, vec!["best"]);
        assert_eq!(selected, "best");
        assert_eq!(variants[0].format, "bv*+ba/b");
        assert_eq!(variants[0].ext, "mp4");
        assert!(
            variants[0].criteria.is_some(),
            "the selection stays semantic"
        );
    }

    #[test]
    fn an_empty_inventory_offers_best_and_mp3_as_before_the_criteria_rewrite() {
        let empty = rd_core::MediaFormatInventory::default();
        let (variants, selected) =
            synthesize_from_inventory(&empty, "720p", MediaCapabilities::complete());
        let offered: Vec<(&str, &str)> = variants
            .iter()
            .map(|v| (v.id.as_str(), v.format.as_str()))
            .collect();
        assert_eq!(offered, vec![("best", "bv*+ba/b"), ("audio_mp3", "ba/b")]);
        assert_eq!(selected, "best");

        // Nothing known means nothing to merge either: without ffmpeg only `b` remains.
        let (variants, _) =
            synthesize_from_inventory(&empty, "best", MediaCapabilities::ytdlp_only());
        let offered: Vec<(&str, &str)> = variants
            .iter()
            .map(|v| (v.id.as_str(), v.format.as_str()))
            .collect();
        assert_eq!(offered, vec![("best", "b")]);
    }

    #[test]
    fn a_page_without_any_progressive_format_offers_nothing_without_ffmpeg() {
        let inventory = normalize(&[video(1080, 900), audio()]);
        let (variants, _) =
            synthesize_from_inventory(&inventory, "best", MediaCapabilities::ytdlp_only());
        assert!(
            variants.is_empty(),
            "offering a preset that cannot download would be a lie: {variants:?}"
        );
    }
}
