//! Whether a state-changing request came from a page of another site (audit 1.9.1, API-01).
//!
//! A browser sends a "simple request" -- a form `POST`, a `text/plain` or `multipart/form-data`
//! body -- to any address without asking first, and carries its ambient credentials along: the
//! session cookie of a same-site page, and nothing at all when the administrator login is
//! switched off for this machine, which is the case that matters. The answer is unreadable to
//! the page, but the request has already run. A page on any site could therefore install a
//! plugin, restore a backup or import site rules through a browser on this machine.
//!
//! The browser says where such a request came from, in two headers no page can set:
//! `Sec-Fetch-Site` and `Origin`. This module reads them; the middleware refuses what they
//! reveal. A request carrying neither is not a browser's, and stays allowed: a command-line
//! client, a script or the updater sends neither, and a cross-site attack needs a browser.
//!
//! The local-network protection newer browsers ship (a public page asking before it reaches a
//! private address) does not cover this: a page served from this machine or the local network
//! is not public, older and other browsers do not ask at all, and a check that depends on the
//! visitor's browser version is not one this service can rely on.

/// Why a request was taken for another site's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Foreign {
    /// The browser said `Sec-Fetch-Site: cross-site`.
    CrossSite,
    /// The `Origin` names neither the host the request was sent to nor the external URL.
    Origin,
}

/// What a state-changing request says about where it came from, or `None` when it is this
/// service's own page or no browser's at all.
///
/// `host` is the request's `Host`; `external_origin` the configured external URL's origin
/// (`https://rd.example.com`), which a proxy that rewrites `Host` leaves as the only name the
/// browser's `Origin` can be compared with.
///
/// `Sec-Fetch-Site: same-origin` is believed outright: no page can set the header, and it is
/// what keeps the interface working behind a proxy that rewrites `Host` without an external URL
/// configured. `same-site` is not enough -- another port on the same host is the same site --
/// so it falls through to the `Origin` comparison, as does a browser that sends no
/// `Sec-Fetch-Site`.
#[must_use]
pub fn foreign_request(
    sec_fetch_site: Option<&str>,
    origin: Option<&str>,
    host: Option<&str>,
    external_origin: Option<&str>,
) -> Option<Foreign> {
    match sec_fetch_site.map(str::trim) {
        Some(site) if site.eq_ignore_ascii_case("cross-site") => return Some(Foreign::CrossSite),
        Some(site) if site.eq_ignore_ascii_case("same-origin") => return None,
        _ => {}
    }
    let origin = origin?.trim();
    if external_origin.is_some_and(|external| {
        external
            .trim_end_matches('/')
            .eq_ignore_ascii_case(origin.trim_end_matches('/'))
    }) {
        return None;
    }
    // An `Origin` with no `Host` to compare it with is vouched for by nothing.
    if host.is_some_and(|host| origin_matches_host(origin, host)) {
        None
    } else {
        Some(Foreign::Origin)
    }
}

/// Whether `origin` (`scheme://host[:port]`) names the same host and port as a `Host` value.
///
/// A `Host` without a port stands for a default one, so the origin's port must be 80 or 443: a
/// proxy forwarding `$host` drops the port, and a page on another port of the same name is
/// exactly what must not pass. `null` -- a sandboxed frame, a `file:` page -- never matches.
fn origin_matches_host(origin: &str, host: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return false,
    };
    let Some((origin_host, origin_port)) = split_host_port(authority) else {
        return false;
    };
    let Some((request_host, request_port)) = split_host_port(host.trim()) else {
        return false;
    };
    if !origin_host.eq_ignore_ascii_case(request_host) {
        return false;
    }
    let origin_port = origin_port.unwrap_or(default_port);
    match request_port {
        Some(port) => port == origin_port,
        None => origin_port == 80 || origin_port == 443,
    }
}

/// `name`, `name:port`, `1.2.3.4:port`, `[::1]` or `[::1]:port`, split; `None` for anything
/// else, a path or a user part included.
fn split_host_port(value: &str) -> Option<(&str, Option<u16>)> {
    if value.is_empty() || value.contains(['/', '@', '?', '#']) {
        return None;
    }
    if let Some(rest) = value.strip_prefix('[') {
        let (inside, after) = rest.split_once(']')?;
        return match after {
            "" => Some((inside, None)),
            _ => Some((inside, Some(after.strip_prefix(':')?.parse().ok()?))),
        };
    }
    match value.split_once(':') {
        None => Some((value, None)),
        Some((name, port)) if !port.contains(':') => Some((name, Some(port.parse().ok()?))),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Foreign, foreign_request};

    #[test]
    fn a_client_that_is_no_browser_is_left_alone() {
        assert_eq!(
            foreign_request(None, None, Some("127.0.0.1:8710"), None),
            None
        );
        assert_eq!(foreign_request(None, None, None, None), None);
    }

    #[test]
    fn the_browser_saying_cross_site_is_refused_whatever_the_origin() {
        assert_eq!(
            foreign_request(
                Some("cross-site"),
                Some("http://127.0.0.1:8710"),
                Some("127.0.0.1:8710"),
                None
            ),
            Some(Foreign::CrossSite)
        );
        assert_eq!(
            foreign_request(Some("Cross-Site"), None, None, None),
            Some(Foreign::CrossSite)
        );
    }

    #[test]
    fn the_interface_itself_passes() {
        assert_eq!(
            foreign_request(
                Some("same-origin"),
                Some("http://127.0.0.1:8710"),
                Some("127.0.0.1:8710"),
                None
            ),
            None
        );
        // An older browser sends no Sec-Fetch-Site; the Origin matching the Host is enough.
        assert_eq!(
            foreign_request(
                None,
                Some("http://localhost:8710"),
                Some("LOCALHOST:8710"),
                None
            ),
            None
        );
        assert_eq!(
            foreign_request(None, Some("http://[::1]:8710"), Some("[::1]:8710"), None),
            None
        );
    }

    #[test]
    fn same_origin_is_believed_behind_a_proxy_that_rewrites_the_host() {
        assert_eq!(
            foreign_request(
                Some("same-origin"),
                Some("https://rd.example.com"),
                Some("127.0.0.1:8710"),
                None
            ),
            None
        );
    }

    #[test]
    fn another_port_of_the_same_host_is_another_site() {
        // Same site to the browser, a different application to this service.
        assert_eq!(
            foreign_request(
                Some("same-site"),
                Some("http://localhost:3000"),
                Some("localhost:8710"),
                None
            ),
            Some(Foreign::Origin)
        );
        assert_eq!(
            foreign_request(None, Some("http://127.0.0.1:3000"), Some("127.0.0.1"), None),
            Some(Foreign::Origin)
        );
    }

    #[test]
    fn a_port_less_host_stands_for_the_default_ports() {
        assert_eq!(
            foreign_request(
                None,
                Some("https://rd.example.com"),
                Some("rd.example.com"),
                None
            ),
            None
        );
        assert_eq!(
            foreign_request(None, Some("http://nas"), Some("nas"), None),
            None
        );
    }

    #[test]
    fn the_external_url_vouches_for_its_own_origin() {
        assert_eq!(
            foreign_request(
                None,
                Some("https://rd.example.com:8443"),
                Some("127.0.0.1:8710"),
                Some("https://rd.example.com:8443")
            ),
            None
        );
        assert_eq!(
            foreign_request(
                None,
                Some("https://evil.example"),
                Some("127.0.0.1:8710"),
                Some("https://rd.example.com")
            ),
            Some(Foreign::Origin)
        );
    }

    #[test]
    fn a_foreign_null_or_malformed_origin_is_refused() {
        for origin in [
            "https://evil.example",
            "null",
            "http://127.0.0.1:8710.evil.example",
            "ftp://127.0.0.1:8710",
            "http://user@127.0.0.1:8710",
            "http://127.0.0.1:8710/path",
        ] {
            assert_eq!(
                foreign_request(None, Some(origin), Some("127.0.0.1:8710"), None),
                Some(Foreign::Origin),
                "{origin}"
            );
        }
        // An Origin with no Host to compare it with is not vouched for either.
        assert_eq!(
            foreign_request(None, Some("http://127.0.0.1:8710"), None, None),
            Some(Foreign::Origin)
        );
    }
}
