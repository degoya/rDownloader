use super::{
    DrmReason, ManifestClass, ManifestKind, MediaRole, detect, parse_dash, parse_hls, same_origin,
};
use url::Url;

fn base() -> Url {
    "https://cdn.example.test/media/master.m3u8"
        .parse()
        .expect("base")
}

const MASTER: &str = "#EXTM3U\n\
        #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",LANGUAGE=\"en\",URI=\"audio/en.m3u8\"\n\
        #EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"sub\",NAME=\"German\",LANGUAGE=\"de\",URI=\"subs/de.m3u8\"\n\
        #EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=1280x720,CODECS=\"avc1.64001f,mp4a.40.2\"\n\
        720p/index.m3u8\n\
        #EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x1080,CODECS=\"avc1.640028\"\n\
        /abs/1080p/index.m3u8\n";

#[test]
fn a_master_playlist_yields_its_variants_with_absolute_urls() {
    let report = parse_hls(MASTER, &base());
    assert_eq!(report.kind, ManifestKind::Hls);
    let video: Vec<_> = report
        .variants
        .iter()
        .filter(|variant| variant.media == MediaRole::Video)
        .collect();
    assert_eq!(video.len(), 2);
    // Relative against the manifest's directory, not against the host root.
    assert_eq!(
        video[0].url.as_str(),
        "https://cdn.example.test/media/720p/index.m3u8"
    );
    assert_eq!(video[0].height, Some(720));
    assert_eq!(video[0].bandwidth, Some(1_280_000));
    // A comma inside a quoted CODECS value must not split the attribute list.
    assert_eq!(video[0].codecs.as_deref(), Some("avc1.64001f,mp4a.40.2"));
    // A root-relative URI resolves against the host.
    assert_eq!(
        video[1].url.as_str(),
        "https://cdn.example.test/abs/1080p/index.m3u8"
    );
}

#[test]
fn audio_and_subtitle_renditions_carry_their_language() {
    let report = parse_hls(MASTER, &base());
    let audio = report
        .variants
        .iter()
        .find(|variant| variant.media == MediaRole::Audio)
        .expect("audio rendition");
    assert_eq!(audio.language.as_deref(), Some("en"));
    let subtitles = report
        .variants
        .iter()
        .find(|variant| variant.media == MediaRole::Subtitles)
        .expect("subtitle rendition");
    assert_eq!(subtitles.language.as_deref(), Some("de"));
    assert_eq!(
        subtitles.url.as_str(),
        "https://cdn.example.test/media/subs/de.m3u8"
    );
}

#[test]
fn a_media_playlist_with_endlist_is_vod_and_without_one_is_live() {
    let vod = "#EXTM3U\n#EXTINF:6.0,\nseg1.ts\n#EXTINF:6.0,\nseg2.ts\n#EXT-X-ENDLIST\n";
    assert_eq!(parse_hls(vod, &base()).class, ManifestClass::Vod);

    // The absence of the tag is the whole signal: a live playlist is simply one that has
    // not ended yet, and treating it as a file produces a download that never completes.
    let live = "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:42\n#EXTINF:6.0,\nseg42.ts\n";
    assert_eq!(parse_hls(live, &base()).class, ManifestClass::Live);
}

#[test]
fn plain_aes_128_is_not_drm() {
    // This is the case that must not regress: ordinary AES-encrypted HLS is extremely
    // common, ffmpeg handles it, and refusing it would break a lot of working streams.
    let body = "#EXTM3U\n\
            #EXT-X-KEY:METHOD=AES-128,URI=\"https://cdn.example.test/key\"\n\
            #EXTINF:6.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
    assert!(parse_hls(body, &base()).drm.is_none());

    let explicit = "#EXTM3U\n\
            #EXT-X-KEY:METHOD=AES-128,KEYFORMAT=\"identity\",URI=\"https://cdn.example.test/key\"\n\
            #EXT-X-ENDLIST\n";
    assert!(parse_hls(explicit, &base()).drm.is_none());

    let none = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXT-X-ENDLIST\n";
    assert!(parse_hls(none, &base()).drm.is_none());
}

#[test]
fn vendor_key_formats_and_sample_aes_are_drm() {
    let widevine = "#EXTM3U\n\
            #EXT-X-SESSION-KEY:METHOD=SAMPLE-AES,KEYFORMAT=\"urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed\"\n\
            #EXT-X-ENDLIST\n";
    assert_eq!(
        parse_hls(widevine, &base()).drm,
        Some(DrmReason::HlsSampleAes)
    );

    let fairplay = "#EXTM3U\n\
            #EXT-X-KEY:METHOD=AES-128,KEYFORMAT=\"com.apple.streamingkeydelivery\",URI=\"skd://x\"\n\
            #EXT-X-ENDLIST\n";
    assert_eq!(
        parse_hls(fairplay, &base()).drm,
        Some(DrmReason::HlsKeyFormat(
            "com.apple.streamingkeydelivery".to_owned()
        ))
    );
}

#[test]
fn a_variant_on_another_host_is_reported_rather_than_silently_trusted() {
    let body = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000\n\
            https://other-cdn.invalid/v/index.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2000\n\
            720p/index.m3u8\n";
    let report = parse_hls(body, &base());
    let foreign = report.foreign_origins(&base());
    assert_eq!(foreign.len(), 1);
    assert_eq!(foreign[0].url.host_str(), Some("other-cdn.invalid"));
}

#[test]
fn origins_compare_scheme_host_and_port() {
    let origin: Url = "https://example.test/a".parse().expect("url");
    assert!(same_origin(
        &"https://example.test/b".parse().expect("url"),
        &origin
    ));
    // The default port is the same origin spelled out.
    assert!(same_origin(
        &"https://example.test:443/b".parse().expect("url"),
        &origin
    ));
    for other in [
        "http://example.test/b",
        "https://example.test:8443/b",
        "https://sub.example.test/b",
    ] {
        assert!(
            !same_origin(&other.parse().expect("url"), &origin),
            "{other}"
        );
    }
}

const MPD: &str = r#"<?xml version="1.0"?>
        <MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static">
          <Period>
            <AdaptationSet contentType="video" mimeType="video/mp4">
              <Representation id="1" bandwidth="1200000" width="1280" height="720" codecs="avc1.4d401f"/>
              <Representation id="2" bandwidth="4000000" width="1920" height="1080" codecs="avc1.640028"/>
            </AdaptationSet>
            <AdaptationSet contentType="audio" lang="en" mimeType="audio/mp4">
              <Representation id="3" bandwidth="128000" codecs="mp4a.40.2"/>
            </AdaptationSet>
          </Period>
        </MPD>"#;

#[test]
fn a_static_mpd_is_vod_and_lists_its_representations() {
    let url: Url = "https://cdn.example.test/m/manifest.mpd"
        .parse()
        .expect("url");
    let report = parse_dash(MPD, &url).expect("parse");
    assert_eq!(report.kind, ManifestKind::Dash);
    assert_eq!(report.class, ManifestClass::Vod);
    assert_eq!(report.variants.len(), 3);
    assert_eq!(report.variants[1].height, Some(1080));
    let audio = &report.variants[2];
    assert_eq!(audio.media, MediaRole::Audio);
    assert_eq!(audio.language.as_deref(), Some("en"));
    assert!(report.drm.is_none());
}

#[test]
fn a_dynamic_mpd_is_live() {
    let url: Url = "https://cdn.example.test/m/manifest.mpd"
        .parse()
        .expect("url");
    let live = MPD.replace(r#"type="static""#, r#"type="dynamic""#);
    assert_eq!(
        parse_dash(&live, &url).expect("parse").class,
        ManifestClass::Live
    );
}

#[test]
fn content_protection_marks_an_mpd_as_drm() {
    let url: Url = "https://cdn.example.test/m/manifest.mpd"
        .parse()
        .expect("url");
    let protected = MPD.replace(
        "<Representation id=\"1\"",
        "<ContentProtection schemeIdUri=\"urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed\"/><Representation id=\"1\"",
    );
    assert_eq!(
        parse_dash(&protected, &url).expect("parse").drm,
        Some(DrmReason::DashContentProtection(
            "urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed".to_owned()
        ))
    );
}

#[test]
fn a_base_url_element_overrides_the_manifest_address() {
    let url: Url = "https://cdn.example.test/m/manifest.mpd"
        .parse()
        .expect("url");
    let with_base = MPD.replace(
        "<Period>",
        "<BaseURL>https://other.example.test/v2/</BaseURL><Period>",
    );
    let report = parse_dash(&with_base, &url).expect("parse");
    assert_eq!(
        report.variants[0].url.as_str(),
        "https://other.example.test/v2/"
    );
    // And that relocation is exactly what the origin check has to notice.
    assert_eq!(report.foreign_origins(&url).len(), 3);
}

#[test]
fn detection_prefers_the_body_over_the_declared_type() {
    let m3u8: Url = "https://cdn.example.test/token/abc".parse().expect("url");
    // A signed CDN URL with no extension, served as text/plain, is still a playlist.
    assert_eq!(
        detect(&m3u8, Some("text/plain; charset=utf-8"), "#EXTM3U\n"),
        Some(ManifestKind::Hls)
    );
    assert_eq!(
        detect(
            &m3u8,
            Some("application/octet-stream"),
            r#"<?xml version="1.0"?><MPD/>"#
        ),
        Some(ManifestKind::Dash)
    );
}

#[test]
fn detection_falls_back_to_the_type_then_the_extension() {
    let bare: Url = "https://cdn.example.test/token/abc".parse().expect("url");
    assert_eq!(
        detect(&bare, Some("application/vnd.apple.mpegurl"), ""),
        Some(ManifestKind::Hls)
    );
    let named: Url = "https://cdn.example.test/v/master.m3u8"
        .parse()
        .expect("url");
    assert_eq!(detect(&named, None, ""), Some(ManifestKind::Hls));
    let dash: Url = "https://cdn.example.test/v/manifest.mpd"
        .parse()
        .expect("url");
    assert_eq!(detect(&dash, None, ""), Some(ManifestKind::Dash));
}

#[test]
fn an_ordinary_file_is_not_a_manifest() {
    let file: Url = "https://cdn.example.test/v/clip.mp4".parse().expect("url");
    assert_eq!(
        detect(&file, Some("video/mp4"), "\u{0}\u{0}\u{0}\u{18}ftyp"),
        None
    );
    let page: Url = "https://example.test/watch".parse().expect("url");
    assert_eq!(
        detect(&page, Some("text/html"), "<!doctype html><html>"),
        None
    );
}

#[test]
fn a_byte_order_mark_does_not_hide_the_tag() {
    let url: Url = "https://cdn.example.test/v/x".parse().expect("url");
    assert_eq!(
        detect(&url, None, "\u{feff}#EXTM3U\n"),
        Some(ManifestKind::Hls)
    );
}
