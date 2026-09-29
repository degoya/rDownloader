//! Where a request came from, as an axum extractor.
//!
//! Thin on purpose: the decision lives in `rd_authn::client_ip`, where it is unit-tested
//! without a server. This is the plumbing that gets the three inputs — the socket's peer
//! address, the two forwarded headers — and the configured proxy list to it.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{Extensions, HeaderMap, request::Parts},
};

use crate::AppState;

/// The address rate limiting and the session inventory attribute a request to.
#[derive(Clone, Copy, Debug)]
pub struct ClientAddress(pub IpAddr);

impl FromRequestParts<AppState> for ClientAddress {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| address.ip())
            // No connect info means the router is being driven directly — every integration
            // test does exactly that. Attributing those to the unspecified address keeps them
            // out of any real address's counters instead of borrowing loopback's.
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let proxy = state.proxy.read().await.clone();
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
        };
        let resolved = rd_authn::resolve_client_address(
            peer,
            header(rd_authn::client_ip::X_FORWARDED_FOR),
            header(rd_authn::client_ip::FORWARDED),
            proxy.trusted(),
        );
        Ok(Self(resolved.address))
    }
}

/// Whether a request came straight from a process on this machine.
///
/// A switched-off administrator login trusts exactly these callers and nobody else (security
/// review 2026-09-28, finding 3): the setting is for a service used from the machine it runs
/// on, and "anyone who reaches the port" was a far wider promise than that — the LAN, a
/// container's port mapping, a reverse proxy's whole audience.
///
/// Two conditions, both about the connection rather than anything the caller claims: the peer
/// is a loopback address, and no forwarding header is present. A reverse proxy on the same
/// machine connects from loopback on behalf of somebody else, and says so in one of those
/// headers; a local client has no reason to send one, and sending one only takes the trust
/// away. No connect info means the router is driven in-process — the tests do that, the
/// service's listener never does (`serve` always attaches it) — and there is no network peer
/// to distrust.
pub fn from_this_machine(extensions: &Extensions, headers: &HeaderMap) -> bool {
    let forwarded = [
        rd_authn::client_ip::X_FORWARDED_FOR,
        rd_authn::client_ip::FORWARDED,
        "x-real-ip",
    ]
    .iter()
    .any(|name| headers.contains_key(*name));
    !forwarded
        && extensions
            .get::<ConnectInfo<SocketAddr>>()
            .is_none_or(|ConnectInfo(address)| address.ip().to_canonical().is_loopback())
}

/// [`from_this_machine`] as an extractor, for a handler that answers differently to it.
#[derive(Clone, Copy, Debug)]
pub struct ThisMachine(pub bool);

impl<S> FromRequestParts<S> for ThisMachine
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(from_this_machine(&parts.extensions, &parts.headers)))
    }
}

/// Strips the configured mount point from the request path before anything routes on it.
///
/// Applied outside the router, so every route pattern, every `MatchedPath` and therefore the
/// whole scope policy stay written as `/api/v1/…` regardless of where the service is mounted.
/// Nesting the router under the base instead would have put the base into `MatchedPath` and
/// silently detached every policy lookup from the route it describes.
///
/// A request that does not start with the base is left alone rather than refused: the health
/// check a container runs, and anything else that reaches the port directly, still works.
pub async fn strip_base_path(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let base = state.proxy.read().await.base_path().to_owned();
    if base.is_empty() {
        return next.run(request).await;
    }
    let path = request.uri().path();
    // `/downloads` and `/downloads/…` both belong to the mount; `/downloadsomething` does not.
    let is_mounted = path == base
        || path
            .strip_prefix(&base)
            .is_some_and(|rest| rest.starts_with('/'));
    if !is_mounted {
        return next.run(request).await;
    }
    let rest = path.strip_prefix(&base).unwrap_or("/");
    let rest = if rest.is_empty() { "/" } else { rest };
    let rebuilt = match request.uri().query() {
        Some(query) => format!("{rest}?{query}"),
        None => rest.to_owned(),
    };
    if let Ok(uri) = rebuilt.parse() {
        *request.uri_mut() = uri;
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    /// The prefix test is the part worth pinning: a base of `/dl` must not swallow `/dlc`.
    ///
    /// Mirrors the condition in [`strip_base_path`], which cannot be called here without a
    /// whole application state.
    fn is_mounted(path: &str, base: &str) -> bool {
        path == base
            || path
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    #[test]
    fn the_mount_point_itself_and_everything_under_it_belong_to_it() {
        assert!(is_mounted("/dl", "/dl"));
        assert!(is_mounted("/dl/", "/dl"));
        assert!(is_mounted("/dl/api/v1/downloads", "/dl"));
    }

    fn connected(address: std::net::IpAddr) -> axum::http::Extensions {
        let mut extensions = axum::http::Extensions::new();
        extensions.insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
            address, 50_000,
        )));
        extensions
    }

    /// Finding 3 of the 2026-09-28 security review: a switched-off login trusted every peer.
    #[test]
    fn only_a_loopback_peer_without_a_forwarding_header_is_this_machine() {
        use axum::http::{HeaderMap, HeaderValue};
        let none = HeaderMap::new();
        for local in ["127.0.0.1", "127.8.9.10", "::1", "::ffff:127.0.0.1"] {
            let address = local.parse().expect("address");
            assert!(
                super::from_this_machine(&connected(address), &none),
                "{local}"
            );
        }
        for remote in [
            "192.168.1.20",
            "10.0.0.2",
            "172.17.0.1",
            "fe80::1",
            "::ffff:10.0.0.2",
        ] {
            let address = remote.parse().expect("address");
            assert!(
                !super::from_this_machine(&connected(address), &none),
                "{remote}"
            );
        }
        let loopback = connected("127.0.0.1".parse().expect("address"));
        for header in ["x-forwarded-for", "forwarded", "x-real-ip"] {
            let mut forwarded = HeaderMap::new();
            forwarded.insert(header, HeaderValue::from_static("203.0.113.9"));
            assert!(
                !super::from_this_machine(&loopback, &forwarded),
                "a proxy on this machine speaks for somebody else: {header}"
            );
        }
    }

    /// The bug this guards against: a sibling path that merely starts with the same letters.
    #[test]
    fn a_path_that_only_begins_with_the_base_is_not_under_it() {
        assert!(!is_mounted("/dlc/import", "/dl"));
        assert!(!is_mounted("/dlx", "/dl"));
        assert!(!is_mounted("/api/v1/dlc/import", "/dl"));
    }
}
