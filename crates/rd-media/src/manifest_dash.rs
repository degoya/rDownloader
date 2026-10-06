//! The DASH half of manifest parsing: the MPD reader and its attribute helpers.

use url::Url;

use super::{DrmReason, ManifestClass, ManifestKind, ManifestReport, ManifestVariant, MediaRole};

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
                let name = element.local_name().as_ref().to_owned();
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
                if let Ok(joined) = base.join(text.trim()) {
                    effective_base = joined;
                }
            }
            Event::End(element) if element.local_name().as_ref() == "BaseURL" => {
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
            let key = attribute.key.local_name().as_ref().to_owned();
            let value = attribute.value.into_owned();
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
