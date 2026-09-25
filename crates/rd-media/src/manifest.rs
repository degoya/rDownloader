//! Direct HLS (`.m3u8`) and MPEG-DASH (`.mpd`) manifests (RD-080-06).
//!
//! A manifest URL used to fall through to the plain HTTP engine, which dutifully downloaded
//! the playlist *text* and called it a video. What this module adds is the classification
//! that has to happen before anything is queued:
//!
//! * **What it is.** Decided from the content type and the body, not from the extension. A
//!   signed CDN URL routinely ends in neither `.m3u8` nor `.mpd`.
//! * **Whether it is live.** An HLS playlist without `#EXT-X-ENDLIST`, or an MPD of
//!   `type="dynamic"`, is a stream with no end; it belongs to the recorder, not to the
//!   file downloader, and the two have completely different completion semantics.
//! * **Whether it is DRM-protected.** That is an explicit non-goal, so it has to be
//!   *detected* and refused with a stable code rather than attempted and failed obscurely.
//!
//! Parsing only. The bytes are fetched by the caller, which owns the HTTP client and the
//! origin rules; keeping this half pure is what makes every case below testable from a
//! fixture instead of from a network.
//!
//! What is deliberately *not* here: a segment downloader. The variants this module finds are
//! handed to yt-dlp/ffmpeg, which already do segment retry and the final remux. A second
//! implementation of that would be a lot of code to arrive back where we started.

use url::Url;

/// Longest manifest accepted. Master playlists are kilobytes; a media playlist for a long
/// VOD can be a few megabytes, and anything past this is not a manifest.
pub const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;

/// Which manifest format was recognised.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestKind {
    Hls,
    Dash,
}

impl ManifestKind {
    /// The `protocol` value the format inventory uses for this kind.
    #[must_use]
    pub const fn protocol(self) -> &'static str {
        match self {
            Self::Hls => "m3u8_native",
            Self::Dash => "dash",
        }
    }
}

/// Whether the manifest describes a finished item or an ongoing stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestClass {
    /// Complete and bounded: `#EXT-X-ENDLIST`, or `MPD@type="static"`.
    Vod,
    /// Open-ended. Routed to the recorder, which has no notion of "100 %".
    Live,
}

/// Why a manifest was judged to be DRM-protected.
///
/// Kept as a reason rather than a bool so the refusal can say *what* it saw; "this stream is
/// protected" with no further detail is the kind of error people file bugs about.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DrmReason {
    /// An HLS key with a non-identity `KEYFORMAT` (Widevine, PlayReady, FairPlay).
    HlsKeyFormat(String),
    /// `METHOD=SAMPLE-AES`, which is FairPlay in practice.
    HlsSampleAes,
    /// A DASH `ContentProtection` element.
    DashContentProtection(String),
}

impl DrmReason {
    /// Short, non-translated detail for the failure's `param`.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::HlsKeyFormat(format) => format.clone(),
            Self::HlsSampleAes => "SAMPLE-AES".to_owned(),
            Self::DashContentProtection(scheme) => scheme.clone(),
        }
    }
}

/// One selectable rendition found in a master manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestVariant {
    /// Absolute URL of the variant playlist or representation.
    pub url: Url,
    pub bandwidth: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Raw `CODECS`/`codecs` value, e.g. `avc1.64001f,mp4a.40.2`.
    pub codecs: Option<String>,
    /// Language of an audio or subtitle rendition.
    pub language: Option<String>,
    pub media: MediaRole,
}

/// What a rendition carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaRole {
    Video,
    Audio,
    Subtitles,
}

/// What one manifest turned out to be.
#[derive(Clone, Debug)]
pub struct ManifestReport {
    pub kind: ManifestKind,
    pub class: ManifestClass,
    /// Empty for a media playlist, which describes segments rather than renditions.
    pub variants: Vec<ManifestVariant>,
    /// Set when the manifest is protected; the caller refuses instead of queueing.
    pub drm: Option<DrmReason>,
}

impl ManifestReport {
    /// Variants whose URL leaves `origin`.
    ///
    /// Not refused outright — a CDN on a second hostname is completely ordinary — but the
    /// caller must not send this manifest's credentials to them. Returned rather than
    /// filtered so the decision stays with the code that holds the credential.
    #[must_use]
    pub fn foreign_origins(&self, origin: &Url) -> Vec<&ManifestVariant> {
        self.variants
            .iter()
            .filter(|variant| !same_origin(&variant.url, origin))
            .collect()
    }
}

/// Whether two URLs share scheme, host and effective port.
#[must_use]
pub fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

/// Recognises a manifest from its content type and its first bytes.
///
/// The content type is checked first because it is what the server actually claims, but it
/// is not trusted alone: plenty of CDNs serve playlists as `text/plain` or
/// `application/octet-stream`, so the body has the final say. The extension is the weakest
/// signal and is only consulted for DASH, whose XML root is otherwise ambiguous.
#[must_use]
pub fn detect(url: &Url, content_type: Option<&str>, body: &str) -> Option<ManifestKind> {
    let content_type = content_type
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();

    // The body is decisive: an `#EXTM3U` tag is not something another format produces.
    if trimmed.starts_with("#EXTM3U") {
        return Some(ManifestKind::Hls);
    }
    if trimmed.contains("<MPD") {
        return Some(ManifestKind::Dash);
    }
    // Only reachable for a truncated or empty body; the declared type then decides.
    match content_type.as_str() {
        "application/vnd.apple.mpegurl" | "application/x-mpegurl" | "audio/mpegurl" => {
            Some(ManifestKind::Hls)
        }
        "application/dash+xml" => Some(ManifestKind::Dash),
        _ => {
            let path = url.path().to_ascii_lowercase();
            if path.ends_with(".m3u8") {
                Some(ManifestKind::Hls)
            } else if path.ends_with(".mpd") {
                Some(ManifestKind::Dash)
            } else {
                None
            }
        }
    }
}

/// Parses an HLS playlist, master or media.
///
/// `base` is the URL the body was actually fetched from — *after* redirects, or every
/// relative URI resolves against the wrong host.
#[must_use]
pub fn parse_hls(body: &str, base: &Url) -> ManifestReport {
    let mut variants = Vec::new();
    let mut drm = None;
    // A media playlist is VOD only if it says so; a master playlist has no `#EXT-X-ENDLIST`
    // of its own, so it is judged by the `#EXT-X-PLAYLIST-TYPE` tag or defaults to VOD once
    // it turns out to carry variants rather than segments.
    let mut has_endlist = false;
    let mut has_segments = false;
    let mut is_master = false;
    let mut pending: Option<PendingStream> = None;

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            is_master = true;
            pending = Some(PendingStream::from_attributes(rest));
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA:") {
            is_master = true;
            if let Some(variant) = media_rendition(rest, base) {
                variants.push(variant);
            }
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("#EXT-X-KEY:")
            .or_else(|| line.strip_prefix("#EXT-X-SESSION-KEY:"))
        {
            drm = drm.or_else(|| key_drm(rest));
            continue;
        }
        if line.starts_with("#EXT-X-ENDLIST") {
            has_endlist = true;
            continue;
        }
        if line.starts_with("#EXTINF") {
            has_segments = true;
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        // A bare line is a URI, belonging to the stream declaration above it when there is
        // one and to the segment list otherwise.
        if let Some(stream) = pending.take()
            && let Ok(url) = base.join(line)
        {
            variants.push(stream.into_variant(url));
        }
    }

    let class = if is_master && !has_segments {
        // A master playlist says nothing about liveness; the caller resolves that from the
        // variant it picks. Treating it as VOD keeps it on the file path, which is right for
        // the overwhelming majority and is corrected when the variant playlist is read.
        ManifestClass::Vod
    } else if has_endlist {
        ManifestClass::Vod
    } else {
        ManifestClass::Live
    };

    ManifestReport {
        kind: ManifestKind::Hls,
        class,
        variants,
        drm,
    }
}

/// A `#EXT-X-STREAM-INF` whose URI is on the following line.
struct PendingStream {
    bandwidth: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    codecs: Option<String>,
}

impl PendingStream {
    fn from_attributes(input: &str) -> Self {
        let attributes = parse_attributes(input);
        let (width, height) = attributes
            .iter()
            .find(|(key, _)| key == "RESOLUTION")
            .and_then(|(_, value)| value.split_once('x'))
            .map_or((None, None), |(w, h)| (w.parse().ok(), h.parse().ok()));
        Self {
            bandwidth: attribute(&attributes, "BANDWIDTH").and_then(|value| value.parse().ok()),
            width,
            height,
            codecs: attribute(&attributes, "CODECS"),
        }
    }

    fn into_variant(self, url: Url) -> ManifestVariant {
        ManifestVariant {
            url,
            bandwidth: self.bandwidth,
            width: self.width,
            height: self.height,
            codecs: self.codecs,
            language: None,
            media: MediaRole::Video,
        }
    }
}

fn media_rendition(input: &str, base: &Url) -> Option<ManifestVariant> {
    let attributes = parse_attributes(input);
    let media = match attribute(&attributes, "TYPE")?.as_str() {
        "AUDIO" => MediaRole::Audio,
        "SUBTITLES" | "CLOSED-CAPTIONS" => MediaRole::Subtitles,
        // VIDEO renditions are alternate angles, which the selector has no vocabulary for.
        _ => return None,
    };
    // A rendition without a URI is served inside the video stream and is not separately
    // fetchable, so there is nothing to offer.
    let uri = attribute(&attributes, "URI")?;
    Some(ManifestVariant {
        url: base.join(&uri).ok()?,
        bandwidth: None,
        width: None,
        height: None,
        codecs: None,
        language: attribute(&attributes, "LANGUAGE"),
        media,
    })
}

/// DRM verdict for one `#EXT-X-KEY`/`#EXT-X-SESSION-KEY` attribute list.
///
/// `METHOD=NONE` is no encryption. Plain `AES-128` with the default (`identity`) key format
/// is ordinary AES that ffmpeg decrypts given the key URI — that is *not* DRM and must not
/// be refused, or a large number of perfectly ordinary streams stop working. Anything with a
/// vendor key format, and `SAMPLE-AES`, is.
fn key_drm(input: &str) -> Option<DrmReason> {
    let attributes = parse_attributes(input);
    let method = attribute(&attributes, "METHOD").unwrap_or_default();
    if method.eq_ignore_ascii_case("NONE") {
        return None;
    }
    if method.eq_ignore_ascii_case("SAMPLE-AES") || method.eq_ignore_ascii_case("SAMPLE-AES-CTR") {
        return Some(DrmReason::HlsSampleAes);
    }
    match attribute(&attributes, "KEYFORMAT") {
        Some(format) if !format.eq_ignore_ascii_case("identity") => {
            Some(DrmReason::HlsKeyFormat(format))
        }
        _ => None,
    }
}

/// Splits `A=1,B="x,y",C=z` honouring quotes, since `CODECS` contains commas.
fn parse_attributes(input: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in input.chars() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                push_pair(&mut pairs, &current);
                current.clear();
            }
            _ => current.push(character),
        }
    }
    push_pair(&mut pairs, &current);
    pairs
}

fn push_pair(pairs: &mut Vec<(String, String)>, raw: &str) {
    if let Some((key, value)) = raw.trim().split_once('=') {
        pairs.push((
            key.trim().to_ascii_uppercase(),
            value.trim().trim_matches('"').to_owned(),
        ));
    }
}

fn attribute(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

/// Parses a DASH MPD.
///
/// Uses the same reader configuration as the NZB and WebDAV parsers: `quick_xml` does not
/// expand external entities unless asked to, which is what keeps a billion-laughs document
/// from being a denial of service.
pub fn parse_dash(body: &str, base: &Url) -> Result<ManifestReport, quick_xml::Error> {
    use quick_xml::{Reader, events::Event};

    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut variants = Vec::new();
    let mut drm = None;
    let mut class = ManifestClass::Vod;
    // The base URL can be overridden by a <BaseURL> element; segment URLs are resolved
    // against it rather than against the manifest address.
    let mut effective_base = base.clone();
    let mut in_base_url = false;
    let mut adaptation_language: Option<String> = None;
    let mut adaptation_role = MediaRole::Video;

    loop {
        match reader.read_event()? {
            Event::Eof => break,
            Event::Start(element) | Event::Empty(element) => {
                let name = element.local_name();
                let name = String::from_utf8_lossy(name.as_ref()).to_string();
                let attributes = xml_attributes(&element);
                match name.as_str() {
                    "MPD" => {
                        if attribute_ci(&attributes, "type")
                            .is_some_and(|value| value.eq_ignore_ascii_case("dynamic"))
                        {
                            class = ManifestClass::Live;
                        }
                    }
                    "BaseURL" => in_base_url = true,
                    "ContentProtection" => {
                        let scheme = attribute_ci(&attributes, "schemeIdUri").unwrap_or_default();
                        // `mp4protection` alone only declares that the stream is encrypted;
                        // a vendor UUID names the system that holds the key. Both mean the
                        // bytes are not decodable here, so both are refused.
                        drm = drm.or(Some(DrmReason::DashContentProtection(scheme)));
                    }
                    "AdaptationSet" => {
                        adaptation_language = attribute_ci(&attributes, "lang");
                        adaptation_role = dash_role(
                            attribute_ci(&attributes, "contentType").as_deref(),
                            attribute_ci(&attributes, "mimeType").as_deref(),
                        );
                    }
                    "Representation" => {
                        let mime = attribute_ci(&attributes, "mimeType");
                        let role = if mime.is_some() {
                            dash_role(None, mime.as_deref())
                        } else {
                            adaptation_role
                        };
                        variants.push(ManifestVariant {
                            // A representation is addressed through its segment template
                            // rather than a URL of its own; the manifest itself is what
                            // yt-dlp is given, so the base stands in as the address.
                            url: effective_base.clone(),
                            bandwidth: attribute_ci(&attributes, "bandwidth")
                                .and_then(|value| value.parse().ok()),
                            width: attribute_ci(&attributes, "width")
                                .and_then(|value| value.parse().ok()),
                            height: attribute_ci(&attributes, "height")
                                .and_then(|value| value.parse().ok()),
                            codecs: attribute_ci(&attributes, "codecs"),
                            language: adaptation_language.clone(),
                            media: role,
                        });
                    }
                    _ => {}
                }
            }
            Event::Text(text) if in_base_url => {
                let value = text.decode().unwrap_or_default();
                if let Ok(joined) = base.join(value.trim()) {
                    effective_base = joined;
                }
            }
            Event::End(element) if element.local_name().as_ref() == b"BaseURL" => {
                in_base_url = false;
            }
            _ => {}
        }
    }

    Ok(ManifestReport {
        kind: ManifestKind::Dash,
        class,
        variants,
        drm,
    })
}

fn dash_role(content_type: Option<&str>, mime: Option<&str>) -> MediaRole {
    let value = content_type
        .or(mime)
        .unwrap_or("video")
        .to_ascii_lowercase();
    if value.contains("audio") {
        MediaRole::Audio
    } else if value.contains("text") || value.contains("ttml") || value.contains("vtt") {
        MediaRole::Subtitles
    } else {
        MediaRole::Video
    }
}

fn xml_attributes(element: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    element
        .attributes()
        .filter_map(Result::ok)
        .map(|attribute| {
            let key = String::from_utf8_lossy(attribute.key.local_name().as_ref()).to_string();
            let value = String::from_utf8_lossy(attribute.value.as_ref()).to_string();
            (key, value)
        })
        .collect()
}

fn attribute_ci(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.clone())
}

#[cfg(test)]
mod tests {
    use super::{
        DrmReason, ManifestClass, ManifestKind, MediaRole, detect, parse_dash, parse_hls,
        same_origin,
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
}
