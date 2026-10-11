use super::{endpoint, folder, updated_body, wanted};

#[test]
fn the_refresh_goes_to_the_server_the_destination_names() {
    assert_eq!(
        endpoint("http://192.168.1.10:8096", false).as_deref(),
        Some("http://192.168.1.10:8096/Library/Refresh")
    );
    assert_eq!(
        endpoint(" https://jellyfin.example.org/ ", false).as_deref(),
        Some("https://jellyfin.example.org/Library/Refresh")
    );
    // A server behind a reverse proxy keeps its base path.
    assert_eq!(
        endpoint("https://example.org/jellyfin/", false).as_deref(),
        Some("https://example.org/jellyfin/Library/Refresh")
    );
}

#[test]
fn a_folder_to_name_goes_to_the_update() {
    assert_eq!(
        endpoint("https://example.org/jellyfin/", true).as_deref(),
        Some("https://example.org/jellyfin/Library/Media/Updated")
    );
}

#[test]
fn a_query_or_fragment_pasted_along_is_dropped() {
    assert_eq!(
        endpoint("https://jellyfin.example.org/#/home.html", false).as_deref(),
        Some("https://jellyfin.example.org/Library/Refresh")
    );
    assert_eq!(
        endpoint("https://jellyfin.example.org/?api_key=pasted", true).as_deref(),
        Some("https://jellyfin.example.org/Library/Media/Updated")
    );
}

#[test]
fn a_destination_that_is_not_an_address_names_no_server() {
    for destination in ["jellyfin", "192.168.1.10:8096", "", "ftp://jellyfin.lan"] {
        assert_eq!(endpoint(destination, false), None, "{destination}");
    }
}

#[test]
fn a_folder_is_named_only_when_the_mapping_holds_it() {
    let mapping = "/downloads=/data/media";
    assert_eq!(
        folder(mapping, Some("/downloads/Holiday")).as_deref(),
        Some("/data/media/Holiday")
    );
    assert_eq!(folder(mapping, None), None);
    assert_eq!(folder(mapping, Some("/elsewhere/Holiday")), None);
    assert_eq!(folder("", Some("/downloads/Holiday")), None);
}

#[test]
fn the_update_names_the_folder_as_created() {
    assert_eq!(
        updated_body("/data/media/Holiday \"2026\""),
        r#"{"Updates":[{"Path":"/data/media/Holiday \"2026\"","UpdateType":"Created"}]}"#
    );
}

#[test]
fn only_a_finished_package_refreshes_the_library() {
    assert!(wanted("package_completed"));
    for event in ["package_failed", "storage_blocked", "update_available", ""] {
        assert!(!wanted(event), "{event}");
    }
}
