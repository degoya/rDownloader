//! The one request a finished package becomes: Jellyfin scans every library.
//!
//! The notifier contract carries no path of the package (RD-1240-12, owner 2026-10-10), so the
//! refresh is the whole library (`/Library/Refresh`) rather than one folder
//! (`/Library/Media/Updated`); a path-precise scan waits for a contract that names it. Jellyfin
//! queues the scan as a task and answers at once.

/// What Jellyfin is asked: scan every library.
pub const REFRESH_PATH: &str = "/Library/Refresh";

/// The `Authorization` value Jellyfin reads its API key from, in the spelling its current
/// versions document. Its older names (`X-Emby-Token`, the `api_key` query) are legacy, and
/// `Authorization` is the only one of them the host lets a plugin set anyway (RD-120-60). The
/// host substitutes the marker; the plugin never sees the key.
pub const AUTHORIZATION: &str = "MediaBrowser Token=\"{{secret}}\"";

/// Whether this event is one a library refresh is for. Only a finished package adds files to
/// the library; a rule that also sends this target other events gets no refresh for them.
#[must_use]
pub fn wanted(event: &str) -> bool {
    event == "package_completed"
}

/// The address to send the refresh to, or `None` when the destination is not the server's
/// address.
///
/// The destination is the server as the person reaches it -- `http://192.168.1.10:8096`, or
/// behind a proxy with a base path of its own. Whether it is an address is read exactly as the
/// host reads it, which narrows the delivery to that address's host (RD-130-15); a query or
/// fragment someone pasted along is dropped, since the refresh is a request of its own.
#[must_use]
pub fn endpoint(destination: &str) -> Option<String> {
    let destination = destination.trim();
    let written_as_address = ["https://", "http://"].iter().any(|scheme| {
        destination
            .get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    if !written_as_address {
        return None;
    }
    let server = destination
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    Some(format!("{server}{REFRESH_PATH}"))
}

#[cfg(test)]
mod tests {
    use super::{endpoint, wanted};

    #[test]
    fn the_refresh_goes_to_the_server_the_destination_names() {
        assert_eq!(
            endpoint("http://192.168.1.10:8096").as_deref(),
            Some("http://192.168.1.10:8096/Library/Refresh")
        );
        assert_eq!(
            endpoint(" https://jellyfin.example.org/ ").as_deref(),
            Some("https://jellyfin.example.org/Library/Refresh")
        );
        // A server behind a reverse proxy keeps its base path.
        assert_eq!(
            endpoint("https://example.org/jellyfin/").as_deref(),
            Some("https://example.org/jellyfin/Library/Refresh")
        );
    }

    #[test]
    fn a_query_or_fragment_pasted_along_is_dropped() {
        assert_eq!(
            endpoint("https://jellyfin.example.org/#/home.html").as_deref(),
            Some("https://jellyfin.example.org/Library/Refresh")
        );
        assert_eq!(
            endpoint("https://jellyfin.example.org/?api_key=pasted").as_deref(),
            Some("https://jellyfin.example.org/Library/Refresh")
        );
    }

    #[test]
    fn a_destination_that_is_not_an_address_names_no_server() {
        for destination in ["jellyfin", "192.168.1.10:8096", "", "ftp://jellyfin.lan"] {
            assert_eq!(endpoint(destination), None, "{destination}");
        }
    }

    #[test]
    fn only_a_finished_package_refreshes_the_library() {
        assert!(wanted("package_completed"));
        for event in ["package_failed", "storage_blocked", "update_available", ""] {
            assert!(!wanted(event), "{event}");
        }
    }
}
