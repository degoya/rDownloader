use super::{endpoint, folder, section, wanted};

#[test]
fn the_refresh_goes_to_the_server_the_destination_names() {
    assert_eq!(
        endpoint("http://192.168.1.10:32400", None).as_deref(),
        Some("http://192.168.1.10:32400/library/sections/all/refresh")
    );
    assert_eq!(
        endpoint(" https://plex.example.org/ ", None).as_deref(),
        Some("https://plex.example.org/library/sections/all/refresh")
    );
    // A server behind a reverse proxy keeps its path.
    assert_eq!(
        endpoint("https://example.org/plex/", None).as_deref(),
        Some("https://example.org/plex/library/sections/all/refresh")
    );
    // The scheme is read case-insensitively, as the host reads it.
    assert_eq!(
        endpoint("HTTPS://plex.example.org", None).as_deref(),
        Some("HTTPS://plex.example.org/library/sections/all/refresh")
    );
}

#[test]
fn a_named_section_is_the_one_refreshed() {
    assert_eq!(
        endpoint("https://plex.example.org", Some("3")).as_deref(),
        Some("https://plex.example.org/library/sections/3/refresh")
    );
}

#[test]
fn a_query_or_fragment_pasted_along_is_dropped() {
    assert_eq!(
        endpoint("https://plex.example.org/web/index.html#!/settings", None).as_deref(),
        Some("https://plex.example.org/web/index.html/library/sections/all/refresh")
    );
    assert_eq!(
        endpoint("https://plex.example.org/?X-Plex-Token=pasted", None).as_deref(),
        Some("https://plex.example.org/library/sections/all/refresh")
    );
}

#[test]
fn a_destination_that_is_not_an_address_names_no_server() {
    for destination in ["plex", "192.168.1.10:32400", "", "ftp://plex.example.org"] {
        assert_eq!(endpoint(destination, None), None, "{destination}");
    }
}

#[test]
fn a_section_is_a_number_or_nothing() {
    assert_eq!(section(None), Ok(None));
    assert_eq!(section(Some(" ")), Ok(None));
    assert_eq!(section(Some(" 12 ")), Ok(Some("12")));
    for refused in ["Movies", "1/2", "-1", "3 4"] {
        assert!(section(Some(refused)).is_err(), "{refused}");
    }
}

#[test]
fn a_folder_is_named_only_within_a_section_and_only_when_mapped() {
    let mapping = "/downloads=/data/media";
    assert_eq!(
        folder(Some("3"), mapping, Some("/downloads/Holiday")).as_deref(),
        Some("/data/media/Holiday")
    );
    // Without a section Plex cannot scan one folder, so the whole library is refreshed.
    assert_eq!(folder(None, mapping, Some("/downloads/Holiday")), None);
    // No folder told, or one the mapping does not hold: the section is refreshed whole.
    assert_eq!(folder(Some("3"), mapping, None), None);
    assert_eq!(folder(Some("3"), mapping, Some("/elsewhere/Holiday")), None);
    assert_eq!(folder(Some("3"), "", Some("/downloads/Holiday")), None);
}

#[test]
fn only_a_finished_package_refreshes_the_library() {
    assert!(wanted("package_completed"));
    for event in ["package_failed", "storage_blocked", "update_available", ""] {
        assert!(!wanted(event), "{event}");
    }
}
