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

#[path = "manifest_dash.rs"]
mod dash;

pub use dash::parse_dash;

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

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
