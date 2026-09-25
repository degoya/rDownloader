//! Properties the format resolver must hold for the selector to be trustworthy.
//!
//! These are the guarantees the UI makes on the resolver's behalf: that what you see is
//! what you get after a restart, that relaxing a filter never shows you *less*, and that an
//! installation without ffmpeg is never offered something it cannot download. Each is
//! checked against real yt-dlp shapes rather than hand-built structs.
//!
//! Permutations are enumerated explicitly rather than through a property-testing crate: the
//! interesting orderings are few, and the fixtures are the real source of variety.

use rd_core::{
    AudioCodecFamily, DynamicRange, LEGACY_PRESETS, MediaFormatCriteria, MediaFormatInventory,
    MediaFormatKind, MediaStrictness, MediaTarget, VideoCodecFamily,
};
use rd_media::{MediaCapabilities, RawFormat, normalize, resolve};

fn inventory(fixture: &str) -> MediaFormatInventory {
    let formats: Vec<RawFormat> =
        serde_json::from_str(fixture).expect("fixture is a yt-dlp format list");
    normalize(&formats)
}

fn youtube() -> MediaFormatInventory {
    inventory(include_str!("fixtures/formats-youtube-av1-hdr.json"))
}

fn arte() -> MediaFormatInventory {
    inventory(include_str!("fixtures/formats-arte-multilang.json"))
}

fn muxed_only() -> MediaFormatInventory {
    inventory(include_str!("fixtures/formats-muxed-only.json"))
}

fn no_audio() -> MediaFormatInventory {
    inventory(include_str!("fixtures/formats-no-audio.json"))
}

fn criteria() -> MediaFormatCriteria {
    MediaFormatCriteria {
        allow_merge: true,
        ..MediaFormatCriteria::default()
    }
}

/// Every rotation of the format list, which is the realistic way a re-probe differs.
fn rotations(inventory: &MediaFormatInventory) -> Vec<MediaFormatInventory> {
    (0..inventory.formats.len())
        .map(|offset| {
            let mut formats = inventory.formats.clone();
            formats.rotate_left(offset);
            MediaFormatInventory {
                formats,
                truncated: inventory.truncated,
            }
        })
        .collect()
}

#[test]
fn resolving_twice_yields_the_same_expression() {
    let inventory = youtube();
    let criteria = MediaFormatCriteria {
        max_height: Some(1080),
        ..criteria()
    };
    let first = resolve(&inventory, &criteria, MediaCapabilities::complete()).expect("resolves");
    let second = resolve(&inventory, &criteria, MediaCapabilities::complete()).expect("resolves");
    assert_eq!(first.format_expression, second.format_expression);
    assert_eq!(first, second);
}

#[test]
fn the_order_the_extractor_listed_formats_in_does_not_matter() {
    // This is what makes "a restart keeps the semantic selection" true: yt-dlp does not
    // promise a stable order, so the ranking must not depend on one.
    for fixture in [youtube(), arte(), muxed_only()] {
        let criteria = criteria();
        let expected = resolve(&fixture, &criteria, MediaCapabilities::complete())
            .expect("resolves")
            .format_expression;
        for rotated in rotations(&fixture) {
            let actual = resolve(&rotated, &criteria, MediaCapabilities::complete())
                .expect("resolves")
                .format_expression;
            assert_eq!(actual, expected, "rotation changed the choice");
        }
    }
}

#[test]
fn relaxing_is_monotone_and_only_happens_when_nothing_matched() {
    let inventory = youtube();
    // AV1 and HDR exist on this page, but never in the same format as VP9.
    let strict = MediaFormatCriteria {
        video_codecs: vec![VideoCodecFamily::Vp9],
        dynamic_range: vec![DynamicRange::Hdr10],
        strictness: MediaStrictness::Required,
        ..criteria()
    };
    let error = resolve(&inventory, &strict, MediaCapabilities::complete())
        .expect_err("VP9 and HDR10 do not co-occur here");
    let rd_core::MediaSelectionError::NoMatch {
        unsatisfiable,
        matched_counts,
        ..
    } = error
    else {
        panic!("expected a no-match error, got {error:?}");
    };
    assert!(
        unsatisfiable.is_empty(),
        "each criterion matches something on its own; only the combination fails"
    );
    assert!(
        matched_counts.iter().all(|entry| entry.matched > 0),
        "the per-criterion counts are what explain the empty result: {matched_counts:?}"
    );

    let preferred = MediaFormatCriteria {
        strictness: MediaStrictness::Preferred,
        ..strict
    };
    let relaxed = resolve(&inventory, &preferred, MediaCapabilities::complete())
        .expect("preferred always resolves to something");
    assert!(
        !relaxed.relaxations.is_empty(),
        "something had to give: {relaxed:?}"
    );
    assert!(!relaxed.is_exact());

    // A criteria set that matches on its own relaxes nothing.
    let satisfiable = MediaFormatCriteria {
        video_codecs: vec![VideoCodecFamily::Avc],
        ..criteria()
    };
    let exact = resolve(&inventory, &satisfiable, MediaCapabilities::complete()).expect("resolves");
    assert!(exact.relaxations.is_empty(), "{exact:?}");
    assert!(exact.is_exact());
    assert!(
        exact.matched_total <= exact.candidate_total,
        "a filter cannot keep more than it was given"
    );
}

#[test]
fn relaxing_a_filter_never_shows_fewer_formats() {
    let inventory = youtube();
    let narrow = MediaFormatCriteria {
        video_codecs: vec![VideoCodecFamily::Av1],
        max_height: Some(1080),
        ..criteria()
    };
    let wide = MediaFormatCriteria {
        video_codecs: Vec::new(),
        ..narrow.clone()
    };
    let narrow = resolve(&inventory, &narrow, MediaCapabilities::complete()).expect("resolves");
    let wide = resolve(&inventory, &wide, MediaCapabilities::complete()).expect("resolves");
    assert!(
        wide.matched_total >= narrow.matched_total,
        "dropping a filter must not shrink the result: {} < {}",
        wide.matched_total,
        narrow.matched_total
    );
}

#[test]
fn without_ffmpeg_the_result_is_always_progressive() {
    // The one rule the whole no-ffmpeg path rests on, checked against a page that offers
    // both progressive and separate streams.
    let inventory = youtube();
    for max_height in [None, Some(2160), Some(1080), Some(480)] {
        let criteria = MediaFormatCriteria {
            max_height,
            allow_merge: false,
            ..criteria()
        };
        let resolution = resolve(&inventory, &criteria, MediaCapabilities::ytdlp_only())
            .expect("the page has a progressive format");
        let video = resolution.video.as_ref().expect("a format was chosen");
        assert_eq!(
            video.kind,
            MediaFormatKind::Muxed,
            "only a progressive format is downloadable without ffmpeg"
        );
        assert!(resolution.audio.is_none(), "nothing to merge");
        assert!(
            !resolution.format_expression.contains('+'),
            "the expression must not ask for a merge: {}",
            resolution.format_expression
        );
    }
}

#[test]
fn a_page_of_separate_streams_reports_that_merging_is_required() {
    let error = resolve(
        &no_audio(),
        &MediaFormatCriteria {
            allow_merge: false,
            ..criteria()
        },
        MediaCapabilities::ytdlp_only(),
    )
    .expect_err("nothing here is downloadable on its own");
    assert_eq!(error.code(), "media.merge_unavailable");

    // The same page has no audio at all, so even *with* ffmpeg there is nothing to merge.
    let error = resolve(&no_audio(), &criteria(), MediaCapabilities::complete())
        .expect_err("there is no audio stream to pair with");
    // Named for what is missing — the audio — rather than blaming an ffmpeg that is
    // installed (RD-120-50).
    assert_eq!(error.code(), "media.audio_missing");
}

#[test]
fn every_legacy_preset_still_resolves_within_its_height_bound() {
    let inventory = youtube();
    for id in LEGACY_PRESETS {
        let criteria = MediaFormatCriteria::preset(id).expect("known preset");
        let criteria = MediaFormatCriteria {
            allow_merge: true,
            ..criteria
        };
        let resolution = resolve(&inventory, &criteria, MediaCapabilities::complete())
            .unwrap_or_else(|error| panic!("{id} must resolve: {error}"));
        let video = resolution.video.as_ref().expect("a format was chosen");
        if let Some(limit) = criteria.max_height {
            assert!(
                video.height.is_none_or(|height| height <= limit),
                "{id} chose {:?}, above its {limit}p bound",
                video.height
            );
            assert!(
                resolution
                    .format_expression
                    .contains(&format!("[height<={limit}]"))
                    || resolution
                        .format_expression
                        .contains(&format!("[height<={}]", video.height.unwrap_or(limit))),
                "{id} lost its height bound: {}",
                resolution.format_expression
            );
        }
        if criteria.target == MediaTarget::AudioOnly {
            assert_eq!(resolution.container, "mp3");
            assert!(
                video.has_audio(),
                "an audio preset must choose a format carrying audio"
            );
        }
        assert!(
            resolution.format_expression.ends_with("/b"),
            "{id} must keep a merge-free last resort: {}",
            resolution.format_expression
        );
    }
}

#[test]
fn a_language_filter_picks_the_matching_audio_track() {
    let inventory = arte();
    let german = MediaFormatCriteria {
        audio_languages: vec!["de".to_owned()],
        ..criteria()
    };
    let resolution = resolve(&inventory, &german, MediaCapabilities::complete())
        .expect("the page has a German track");
    let audio = resolution.audio.as_ref().expect("a track was merged on");
    assert_eq!(audio.language.as_deref(), Some("de"));
    assert_eq!(
        audio.audio_bitrate_kbps,
        Some(128),
        "the richer of the two German tracks"
    );

    // `de` also matches `de-DE`, but not `fr`.
    let french = MediaFormatCriteria {
        audio_languages: vec!["fr".to_owned()],
        ..criteria()
    };
    let resolution = resolve(&inventory, &french, MediaCapabilities::complete())
        .expect("the page has a French track");
    assert_eq!(
        resolution
            .audio
            .as_ref()
            .and_then(|format| format.language.as_deref()),
        Some("fr")
    );
}

#[test]
fn an_impossible_audio_filter_still_produces_a_playable_file() {
    // Losing the audio track entirely because a codec filter matched nothing would be a
    // worse outcome than ignoring the filter, so the audio side relaxes silently.
    let resolution = resolve(
        &arte(),
        &MediaFormatCriteria {
            audio_codecs: vec![AudioCodecFamily::Flac],
            ..criteria()
        },
        MediaCapabilities::complete(),
    )
    .expect("resolves");
    assert!(
        resolution.audio.is_some(),
        "a video without sound is not what anyone asked for"
    );
}

#[test]
fn the_expression_pins_the_chosen_ids_first() {
    let resolution =
        resolve(&youtube(), &criteria(), MediaCapabilities::complete()).expect("resolves");
    let video = resolution.video.as_ref().expect("video");
    let audio = resolution.audio.as_ref().expect("audio");
    let alternatives: Vec<&str> = resolution.format_expression.split('/').collect();
    assert_eq!(
        alternatives.first().copied(),
        Some(format!("{}+{}", video.format_id, audio.format_id).as_str()),
        "a still-valid id must win before any re-resolution: {alternatives:?}"
    );
    assert_eq!(
        alternatives.last().copied(),
        Some("b"),
        "the last resort needs no ffmpeg: {alternatives:?}"
    );
}

#[test]
fn an_empty_inventory_is_reported_rather_than_guessed_at() {
    let error = resolve(
        &MediaFormatInventory::default(),
        &criteria(),
        MediaCapabilities::complete(),
    )
    .expect_err("nothing to choose from");
    assert_eq!(error.code(), "media.formats_missing");
}
