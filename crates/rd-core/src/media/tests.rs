use super::{MEDIA_CONTRACT_VERSION, MediaKind, MediaSelection, MediaSettings, MediaTarget};

/// A `downloads.media_json` blob exactly as rDownloader 0.6 wrote it.
const LEGACY_SELECTION: &str = r#"{
        "page_url": "https://www.youtube.com/watch?v=abc",
        "variant_id": "1080p",
        "format": "bv*[height<=1080]+ba/b[height<=1080]",
        "kind": "video",
        "ext": "mp4",
        "title": "clip"
    }"#;

#[test]
fn a_pre_selector_row_still_loads_and_resolves_through_its_preset() {
    let selection: MediaSelection =
        serde_json::from_str(LEGACY_SELECTION).expect("legacy blob still deserialises");
    assert_eq!(selection.contract_version, 0);
    assert!(!selection.is_future_contract());
    assert_eq!(selection.criteria, None);
    assert_eq!(selection.resolved, None);
    // The stored expression is untouched, so a row mid-download keeps working even if
    // nothing re-resolves it.
    assert_eq!(selection.format, "bv*[height<=1080]+ba/b[height<=1080]");

    let criteria = selection
        .effective_criteria()
        .expect("a preset id resolves to criteria");
    assert_eq!(criteria.max_height, Some(1080));
    assert_eq!(criteria.preset.as_deref(), Some("1080p"));
    assert_eq!(criteria.target, MediaTarget::Video);
}

#[test]
fn a_recording_row_has_no_criteria_to_resolve() {
    // The stream monitor puts a streamlink quality in `format`; treating it as an
    // extractor expression would break every livestream recording.
    let selection = MediaSelection {
        page_url: "https://twitch.tv/example".parse().expect("url"),
        variant_id: "720p60".to_owned(),
        format: "720p60".to_owned(),
        kind: MediaKind::Video,
        ext: "ts".to_owned(),
        title: "stream".to_owned(),
        contract_version: MEDIA_CONTRACT_VERSION,
        criteria: None,
        resolved: None,
    };
    assert_eq!(selection.effective_criteria(), None);
}

#[test]
fn a_blob_from_a_newer_version_is_detected_rather_than_misread() {
    let mut blob: serde_json::Value = serde_json::from_str(LEGACY_SELECTION).expect("legacy blob");
    blob["contract_version"] = serde_json::json!(MEDIA_CONTRACT_VERSION + 1);
    let selection: MediaSelection = serde_json::from_value(blob).expect("unknown fields default");
    assert!(selection.is_future_contract());
}

#[test]
fn host_matching_ignores_www_and_case() {
    let settings = MediaSettings::default();
    assert!(settings.handles_host("www.YouTube.com"));
    // The one host form (RA-IN-06): a trailing dot is the same host.
    assert!(settings.handles_host("WWW.YouTube.com."));
    assert!(settings.handles_host("youtu.be"));
    assert!(settings.handles_host("dumpert.nl"));
    assert!(!settings.handles_host("notyoutube.com"));
}
