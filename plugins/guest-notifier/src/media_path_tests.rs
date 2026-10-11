//! The folder mapping of the media servers (RD-1250-01): prefix, longest match, no match.

use super::{json_string, mapped};

const MAPPING: &str = "/srv/downloads=/data/media\n/srv/downloads/series = /data/tv\n";

#[test]
fn a_folder_below_a_service_path_moves_below_its_server_path() {
    assert_eq!(
        mapped(MAPPING, "/srv/downloads/Holiday").as_deref(),
        Some("/data/media/Holiday")
    );
    assert_eq!(
        mapped(MAPPING, "/srv/downloads/Holiday/Disc 1").as_deref(),
        Some("/data/media/Holiday/Disc 1")
    );
    // The service path itself is its server path.
    assert_eq!(
        mapped(MAPPING, "/srv/downloads").as_deref(),
        Some("/data/media")
    );
}

#[test]
fn the_longest_service_path_that_holds_the_folder_wins() {
    assert_eq!(
        mapped(MAPPING, "/srv/downloads/series/Show S01").as_deref(),
        Some("/data/tv/Show S01")
    );
    // Whatever the order of the lines.
    let reversed = "/srv/downloads/series=/data/tv\n/srv/downloads=/data/media";
    assert_eq!(
        mapped(reversed, "/srv/downloads/series/Show S01").as_deref(),
        Some("/data/tv/Show S01")
    );
}

#[test]
fn a_folder_no_line_holds_is_no_folder_to_name() {
    for path in [
        "/srv/downloads2/Holiday",
        "/srv/download",
        "/home/me/Holiday",
        "srv/downloads/Holiday",
        "",
    ] {
        assert_eq!(mapped(MAPPING, path), None, "{path:?}");
    }
    // No mapping at all, or none that is one.
    for mapping in [
        "",
        "\n\n",
        "/srv/downloads",
        "=/data/media",
        "/srv/downloads=",
    ] {
        assert_eq!(
            mapped(mapping, "/srv/downloads/Holiday"),
            None,
            "{mapping:?}"
        );
    }
}

#[test]
fn separators_at_the_end_of_a_line_do_not_matter() {
    let mapping = "/srv/downloads/ = /data/media/";
    assert_eq!(
        mapped(mapping, "/srv/downloads/Holiday").as_deref(),
        Some("/data/media/Holiday")
    );
    assert_eq!(
        mapped("/=/mnt/root", "/Holiday").as_deref(),
        Some("/mnt/root/Holiday")
    );
}

#[test]
fn a_windows_service_path_maps_case_blind_onto_the_server_separator() {
    let mapping = "D:\\Downloads=/data/media\r\n";
    assert_eq!(
        mapped(mapping, "d:\\downloads\\Holiday\\Disc 1").as_deref(),
        Some("/data/media/Holiday/Disc 1")
    );
    let to_windows = "/srv/downloads=\\\\nas\\media";
    assert_eq!(
        mapped(to_windows, "/srv/downloads/Holiday/Disc 1").as_deref(),
        Some("\\\\nas\\media\\Holiday\\Disc 1")
    );
    // A Unix path stays case-sensitive.
    assert_eq!(mapped(MAPPING, "/SRV/downloads/Holiday"), None);
}

/// A folder whose name held a brace reached the plugin with the host's inert one in its place:
/// it names no folder on disk, so none is named.
#[test]
fn a_folder_whose_brace_was_made_inert_is_not_named() {
    assert_eq!(
        mapped(MAPPING, "/srv/downloads/Holiday \u{2774}2026}"),
        None
    );
}

#[test]
fn a_folder_travels_in_json_intact() {
    assert_eq!(
        json_string("/data/media/Holiday"),
        "\"/data/media/Holiday\""
    );
    assert_eq!(
        json_string("C:\\Media\\\"Best\" of\n"),
        "\"C:\\\\Media\\\\\\\"Best\\\" of\\n\""
    );
    assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
}
