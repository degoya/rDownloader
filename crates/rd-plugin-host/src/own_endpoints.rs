//! The service's own listeners, which no plugin request may reach (RA-HOST-01, owner's
//! decision 2026-10-04).
//!
//! An address the person entered — a storage destination, a crawled address, a notification
//! target — may name a server on this machine, loopback included. What it may never reach is
//! rDownloader itself: the web interface and API, which trusts a request from this machine, and
//! the capture agent's Click'n'Load listener. A plugin's reach is a host, not a port, so a
//! target on `localhost:2586` would otherwise also have reached `localhost:8710`.

use std::{
    net::SocketAddr,
    sync::{LazyLock, PoisonError, RwLock},
};

/// The Click'n'Load port the capture agent listens on — `rdownloader-capture --cnl-listen`,
/// `127.0.0.1:9666` and `[::1]:9666` unless told otherwise, the port every Click'n'Load page
/// posts to.
pub const CLICK_N_LOAD_PORT: u16 = 9666;

/// Where this service and its companions listen.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnEndpoints {
    listen: Option<SocketAddr>,
    ports: Vec<u16>,
}

impl OwnEndpoints {
    /// The service listening on `listen`, and the capture agent on [`CLICK_N_LOAD_PORT`].
    ///
    /// The torrent session's peer port is not among them: it speaks the peer protocol and
    /// trusts nobody for being local, so reaching it from a plugin gains nothing.
    #[must_use]
    pub fn new(listen: Option<SocketAddr>) -> Self {
        let mut ports = vec![CLICK_N_LOAD_PORT];
        ports.extend(listen.map(|listen| listen.port()));
        ports.sort_unstable();
        ports.dedup();
        Self { listen, ports }
    }

    /// The address the service's web interface and API listen on, if known.
    #[must_use]
    pub fn listen(&self) -> Option<SocketAddr> {
        self.listen
    }

    /// Every port that is one of ours.
    #[must_use]
    pub fn ports(&self) -> &[u16] {
        &self.ports
    }

    /// Whether `port` is one of ours.
    #[must_use]
    pub fn is_own_port(&self, port: Option<u16>) -> bool {
        port.is_some_and(|port| self.ports.contains(&port))
    }

    /// Which addresses a plugin request to `url` may reach.
    ///
    /// Public addresses always. Where the person entered the address (`entered`), their own
    /// network and this machine's loopback as well — a WebDAV server or an ntfy beside the
    /// service is an ordinary setup. Never link-local, with a cloud's metadata endpoint, nor
    /// anything else [`rd_core::address_scope`] calls local. And never the service itself: on
    /// one of its own ports loopback is refused and so is every address the service listens
    /// on, whoever entered it, and no redirect may lead to one of those ports.
    pub(crate) fn policy(&self, entered: bool, url: &url::Url) -> rd_http::AddressPolicy {
        let policy = rd_http::AddressPolicy::new(entered).refusing_redirects_to(&self.ports);
        if self.is_own_port(url.port_or_known_default()) {
            policy.listening_on(self.listen)
        } else if entered {
            policy.with_loopback()
        } else {
            policy
        }
    }

    /// The refusal of `url` by [`Self::policy`]: a code of its own where the port is one of
    /// ours, so the person reads which rule it was.
    pub(crate) fn refused(&self, url: &url::Url) -> rd_core::Failure {
        if self.is_own_port(url.port_or_known_default()) {
            return rd_core::Failure::coded(
                rd_core::FailureKind::Permanent,
                "plugin.http_own_service",
                "The plugin request points at one of rDownloader's own services",
            );
        }
        rd_core::Failure::coded(
            rd_core::FailureKind::Permanent,
            "plugin.http_local_target",
            "The plugin request points at this machine or a local network it may not reach",
        )
    }
}

/// The endpoints of the service running in this process, for the decisions made without a
/// host in hand — whether a notification target may be saved. Set by
/// [`crate::ResolverService::new`]; until then only Click'n'Load's port is known.
static CURRENT: LazyLock<RwLock<OwnEndpoints>> =
    LazyLock::new(|| RwLock::new(OwnEndpoints::new(None)));

pub(crate) fn publish(own: &OwnEndpoints) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = own.clone();
}

pub(crate) fn current() -> OwnEndpoints {
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::{CLICK_N_LOAD_PORT, OwnEndpoints};

    #[test]
    fn the_listen_port_and_click_n_load_are_ours() {
        let own = OwnEndpoints::new(Some("0.0.0.0:8710".parse().expect("address")));
        assert_eq!(own.ports(), [8710, CLICK_N_LOAD_PORT]);
        assert!(own.is_own_port(Some(8710)));
        assert!(own.is_own_port(Some(9666)));
        assert!(!own.is_own_port(Some(2586)));
        assert!(!own.is_own_port(None));
        assert_eq!(OwnEndpoints::new(None).ports(), [CLICK_N_LOAD_PORT]);
    }
}
