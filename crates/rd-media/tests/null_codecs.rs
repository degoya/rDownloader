//! RD-120-50: pages whose formats yt-dlp reports without codecs.
//!
//! Both fixtures are the real `yt-dlp -J` answers of 2026-09-24 (yt-dlp 2026.08.19), trimmed
//! to the fields the normaliser reads: arte.tv lists its HLS audio renditions with
//! `acodec: null`, dumpert.nl every format with both codecs `null` plus eight picture and
//! subtitle pseudo-formats. Until this job the normaliser read `null` like `"none"`, so arte
//! lost every audio track and dumpert every format — and since `a2beadf6` there was no
//! `best` fallback left to catch it, so the link was queued without a selection.

use rd_core::{MediaFormatCriteria, MediaFormatInventory, MediaFormatKind, MediaTarget};
use rd_media::{
    MediaCapabilities, RawFormat, normalize, resolve, synthesize_from_inventory,
    synthesize_variants,
};

fn raw(fixture: &str) -> Vec<RawFormat> {
    serde_json::from_str(fixture).expect("fixture is a yt-dlp format list")
}

fn dumpert() -> Vec<RawFormat> {
    raw(include_str!(
        "fixtures/formats-dumpert-hls-null-codecs.json"
    ))
}

fn arte() -> Vec<RawFormat> {
    raw(include_str!("fixtures/formats-arte-hls-null-acodec.json"))
}

fn best() -> MediaFormatCriteria {
    MediaFormatCriteria::preset("best").expect("best is a preset")
}

/// The regression `a2beadf6` introduced: dumpert.nl downloaded with 0.7.0 and offered
/// nothing from 0.8.0 on.
#[test]
fn regression_a2beadf6_dumpert_offers_best_again() {
    let (variants, selected) = synthesize_variants(&dumpert(), "best");
    let ids: Vec<&str> = variants.iter().map(|variant| variant.id.as_str()).collect();
    assert_eq!(ids, vec!["best", "1080p", "720p", "480p", "audio_mp3"]);
    assert_eq!(selected, "best");

    let best = &variants[0];
    // The tallest rendition, pinned first, and nothing asking for a merge: every format
    // here carries both tracks as far as anybody knows. This exact string was handed to
    // the real yt-dlp against the reported page; the numbers are in the job file.
    assert_eq!(best.format, "stream-3401/b[height<=1280]/b");
    assert_eq!(best.height, Some(1280));
    assert!(!best.requires_merge);
    assert_eq!(best.ext, "mp4");
}

#[test]
fn dumpert_keeps_its_six_streams_and_drops_its_pictures() {
    let inventory = normalize(&dumpert());
    let ids: Vec<&str> = inventory
        .formats
        .iter()
        .map(|format| format.format_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "stream-1094",
            "stream-2176",
            "stream-3401",
            "mobile",
            "tablet",
            "720p"
        ]
    );
    assert!(
        inventory
            .formats
            .iter()
            .all(|format| format.kind == MediaFormatKind::Muxed),
        "both codecs null is a muxed format with unknown codecs"
    );
    // Muxed formats need no ffmpeg, so an installation without one still gets them.
    let resolution = resolve(&inventory, &best(), MediaCapabilities::ytdlp_only())
        .expect("a progressive format is reachable without ffmpeg");
    assert_eq!(
        resolution.video.as_ref().map(|f| f.format_id.as_str()),
        Some("stream-3401")
    );
}

#[test]
fn arte_merges_the_best_video_with_an_audio_track() {
    let inventory = normalize(&arte());
    assert_eq!(inventory.of_kind(MediaFormatKind::Video).count(), 6);
    assert_eq!(
        inventory.of_kind(MediaFormatKind::Audio).count(),
        5,
        "`acodec: null` is an unknown codec, not a missing track"
    );
    let resolution =
        resolve(&inventory, &best(), MediaCapabilities::complete()).expect("best resolves");
    let video = resolution.video.as_ref().expect("a video format");
    let audio = resolution.audio.as_ref().expect("an audio track to merge");
    assert_eq!(video.height, Some(1080));
    assert_eq!(video.format_id, "VOA-2222");
    assert_eq!(audio.format_id, "VOA-audio_0-Deutsch");
    assert_eq!(
        resolution.format_expression,
        "VOA-2222+VOA-audio_0-Deutsch/bv*[height<=1080][vcodec^=avc1]+ba/b[height<=1080]/b"
    );

    let (variants, selected) = synthesize_variants(&arte(), "720p");
    let ids: Vec<&str> = variants.iter().map(|variant| variant.id.as_str()).collect();
    assert_eq!(ids, vec!["best", "1080p", "720p", "480p", "audio_mp3"]);
    assert_eq!(selected, "720p");
    assert!(variants[0].requires_merge);
}

#[test]
fn an_audio_language_filter_picks_among_the_null_codec_tracks() {
    let criteria = MediaFormatCriteria {
        audio_languages: vec!["fr".to_owned()],
        ..best()
    };
    let resolution = resolve(
        &normalize(&arte()),
        &criteria,
        MediaCapabilities::complete(),
    )
    .expect("resolves");
    let audio = resolution.audio.expect("an audio track");
    assert_eq!(audio.language.as_deref(), Some("fr"));
}

#[test]
fn arte_without_ffmpeg_still_offers_nothing_it_cannot_download() {
    // What `a2beadf6` fixed and this job keeps: arte serves separate streams only, so an
    // installation that cannot merge is offered no preset — not even the fallback.
    let inventory = normalize(&arte());
    let (variants, _) =
        synthesize_from_inventory(&inventory, "best", MediaCapabilities::ytdlp_only());
    assert!(variants.is_empty(), "{variants:?}");
}

#[test]
fn audio_only_on_dumpert_extracts_from_a_muxed_stream() {
    let criteria = MediaFormatCriteria::preset("audio_mp3").expect("preset");
    assert_eq!(criteria.target, MediaTarget::AudioOnly);
    let resolution = resolve(
        &normalize(&dumpert()),
        &criteria,
        MediaCapabilities::complete(),
    )
    .expect("resolves");
    assert_eq!(resolution.container, "mp3");
}

#[test]
fn an_empty_inventory_is_no_evidence_against_anything() {
    let (variants, selected) = synthesize_from_inventory(
        &MediaFormatInventory::default(),
        "best",
        MediaCapabilities::ytdlp_only(),
    );
    assert_eq!(selected, "best");
    assert_eq!(variants.len(), 1);
    assert_eq!(variants[0].format, "b", "nothing may ask for a merge");
}
