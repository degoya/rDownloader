//! Where the headers of an HTTP authentication profile may go (RD-120-43).
//!
//! A profile is chosen for the address a download was added with, and its `Authorization`
//! header rides along per request. reqwest drops that header when a redirect changes host, but
//! the worker then fetches the chunks from where the probe's redirects *ended*, directly — and a
//! resolver may answer with an address on another host altogether. Neither may take the header
//! past the profile's scope, so it is decided again for every address a request goes to: the
//! same treatment RD-120-38 gave the account's own credential.
//!
//! Cookies of a profile need none of this. They sit in the client's jar bound to their domain,
//! and the jar decides per request, redirect or not.

use rd_core::AuthScope;
use url::Url;

/// The boundary a profile's per-request headers are confined to.
#[derive(Clone, Debug)]
pub struct ProfileBoundary {
    scope: AuthScope,
    /// Whether the address the profile was chosen for is HTTPS. A header chosen for an HTTPS
    /// address is never sent in the clear, which is also what reqwest does with a redirect
    /// from HTTPS to HTTP.
    chosen_over_tls: bool,
}

impl ProfileBoundary {
    /// The boundary of a profile with `scope`, chosen for the address `chosen_for`.
    #[must_use]
    pub fn new(scope: AuthScope, chosen_for: &Url) -> Self {
        Self {
            scope,
            chosen_over_tls: chosen_for.scheme() == "https",
        }
    }

    /// Whether a request to `target` may carry the profile's headers.
    ///
    /// The host decides, not the path prefix: the prefix selects *which* profile applies and is
    /// no containment boundary (see [`AuthScope`]) — a redirect to another path of the same host
    /// keeps the header, as it did in the probe.
    #[must_use]
    pub fn admits(&self, target: &Url) -> bool {
        let host_in_scope = target
            .host_str()
            .is_some_and(|host| self.scope.matches_host(host));
        host_in_scope && (!self.chosen_over_tls || target.scheme() == "https")
    }
}

/// The profile headers a request to `target` may carry: all of them inside the boundary, none
/// outside it, and none without a boundary.
#[must_use]
pub(crate) fn admitted(
    headers: &[(String, String)],
    boundary: Option<&ProfileBoundary>,
    target: &Url,
) -> Vec<(String, String)> {
    if boundary.is_some_and(|boundary| boundary.admits(target)) {
        headers.to_vec()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ProfileBoundary, admitted};

    fn url(text: &str) -> url::Url {
        text.parse().expect("url")
    }

    fn boundary(scope: &str, subdomains: bool, chosen_for: &str) -> ProfileBoundary {
        ProfileBoundary::new(
            rd_core::AuthScope::parse(scope, subdomains).expect("scope"),
            &url(chosen_for),
        )
    }

    fn header() -> Vec<(String, String)> {
        vec![("authorization".to_owned(), "Bearer token".to_owned())]
    }

    #[test]
    fn a_foreign_host_gets_nothing() {
        let boundary = boundary("a.example", false, "https://a.example/file");
        let headers = header();
        assert_eq!(
            admitted(&headers, Some(&boundary), &url("https://a.example/file")),
            headers
        );
        assert!(admitted(&headers, Some(&boundary), &url("https://b.example/file")).is_empty());
        // The leading dot of the scope rule, not a suffix match.
        assert!(admitted(&headers, Some(&boundary), &url("https://evil-a.example/")).is_empty());
    }

    #[test]
    fn subdomains_follow_the_scope_setting() {
        let target = url("https://cdn.a.example/file");
        assert!(boundary("a.example", true, "https://a.example/").admits(&target));
        assert!(!boundary("a.example", false, "https://a.example/").admits(&target));
    }

    #[test]
    fn the_path_prefix_is_no_boundary() {
        let boundary = boundary(
            "https://a.example/media/",
            false,
            "https://a.example/media/x",
        );
        assert!(boundary.admits(&url("https://a.example/cdn/x")));
    }

    #[test]
    fn a_header_chosen_over_tls_is_never_sent_in_the_clear() {
        assert!(
            !boundary("a.example", false, "https://a.example/").admits(&url("http://a.example/"))
        );
        // A plain source may go on to HTTPS, and stays plain where it was plain.
        let plain = boundary("a.example", false, "http://a.example/");
        assert!(plain.admits(&url("https://a.example/")));
        assert!(plain.admits(&url("http://a.example/")));
    }

    #[test]
    fn no_boundary_admits_nothing() {
        assert!(admitted(&header(), None, &url("https://a.example/")).is_empty());
    }
}
