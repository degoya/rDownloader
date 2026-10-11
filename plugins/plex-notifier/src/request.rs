//! The one request a finished package becomes: Plex scans every library section.
//!
//! The notifier contract carries no path of the package (RD-1240-12, owner 2026-10-10), so the
//! refresh is the whole library rather than one folder; a path-precise scan waits for a contract
//! that names it. Plex starts the scan in the background and answers at once, so a refresh per
//! finished package costs the server a scan, not a request that waits for one.

/// What Plex is asked: scan every section. `all` is Plex's own spelling for that.
pub const REFRESH_PATH: &str = "/library/sections/all/refresh";

/// The query name Plex reads its token from. The host allows no header of that name
/// (RD-120-60), and Plex reads the query as readily.
pub const TOKEN_QUERY: &str = "X-Plex-Token";

/// Whether this event is one a library refresh is for. Only a finished package adds files to
/// the library; a rule that also sends this target other events gets no refresh for them.
#[must_use]
pub fn wanted(event: &str) -> bool {
    event == "package_completed"
}

/// The address to send the refresh to, or `None` when the destination is not the server's
/// address.
///
/// The destination is the server as the person reaches it -- `http://192.168.1.10:32400`, or
/// behind a proxy with a path of its own. Whether it is an address is read exactly as the host
/// reads it, which narrows the delivery to that address's host (RD-130-15); a query or fragment
/// someone pasted along is dropped, since the refresh is a request of its own.
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
            endpoint("http://192.168.1.10:32400").as_deref(),
            Some("http://192.168.1.10:32400/library/sections/all/refresh")
        );
        assert_eq!(
            endpoint(" https://plex.example.org/ ").as_deref(),
            Some("https://plex.example.org/library/sections/all/refresh")
        );
        // A server behind a reverse proxy keeps its path.
        assert_eq!(
            endpoint("https://example.org/plex/").as_deref(),
            Some("https://example.org/plex/library/sections/all/refresh")
        );
        // The scheme is read case-insensitively, as the host reads it.
        assert_eq!(
            endpoint("HTTPS://plex.example.org").as_deref(),
            Some("HTTPS://plex.example.org/library/sections/all/refresh")
        );
    }

    #[test]
    fn a_query_or_fragment_pasted_along_is_dropped() {
        assert_eq!(
            endpoint("https://plex.example.org/web/index.html#!/settings").as_deref(),
            Some("https://plex.example.org/web/index.html/library/sections/all/refresh")
        );
        assert_eq!(
            endpoint("https://plex.example.org/?X-Plex-Token=pasted").as_deref(),
            Some("https://plex.example.org/library/sections/all/refresh")
        );
    }

    #[test]
    fn a_destination_that_is_not_an_address_names_no_server() {
        for destination in ["plex", "192.168.1.10:32400", "", "ftp://plex.example.org"] {
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
