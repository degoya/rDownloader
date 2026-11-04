//! The service's own listeners, which no plugin request may reach (RA-HOST-01, owner's
//! decision 2026-10-04).
//!
//! An address the person entered — a storage destination, a crawled address, a notification
//! target — may name a server on this machine, loopback included. What it may never reach is
//! rDownloader itself: the web interface and API, which trusts a request from this machine, and
//! the capture agent's Click'n'Load listener. A plugin's reach is a host, not a port, so a
//! target on `localhost:2586` would otherwise also have reached `localhost:8710`.
//!
//! Behind a reverse proxy on the same machine the service also answers on the proxy's port —
//! the port of the configured external URL, mostly 443 or 80 — so on this machine that port is
//! ours as well (RD-1420-01, audit L-10). Only there: on any other host 443 is every web
//! server's, so a redirect to it is not refused the way one to the listen port is.

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
    /// The port a reverse proxy on this machine passes on to the service: the external URL's.
    proxy_ports: Vec<u16>,
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
        Self {
            listen,
            ports,
            proxy_ports: Vec::new(),
        }
    }

    /// The address the service's web interface and API listen on, if known.
    #[must_use]
    pub fn listen(&self) -> Option<SocketAddr> {
        self.listen
    }

    /// The ports our own listeners have on every address: the listen port and Click'n'Load's.
    /// No redirect may lead to one of them, whatever the host.
    #[must_use]
    pub fn ports(&self) -> &[u16] {
        &self.ports
    }

    /// Also the port of `external_url`, what the outside world calls this service: a reverse
    /// proxy in front of it on this machine answers there and passes the request on to the
    /// service (RD-1420-01). The port is the URL's own or its scheme's default. It is ours on
    /// this machine only, so it joins [`Self::is_own_port`] and not [`Self::ports`].
    #[must_use]
    pub fn with_external_url(mut self, external_url: Option<&url::Url>) -> Self {
        self.proxy_ports = external_url
            .and_then(url::Url::port_or_known_default)
            .into_iter()
            .collect();
        self
    }

    /// Whether `port` is one of ours on this machine: a listener's, or the reverse proxy's.
    #[must_use]
    pub(crate) fn is_own_port(&self, port: Option<u16>) -> bool {
        port.is_some_and(|port| self.ports.contains(&port) || self.proxy_ports.contains(&port))
    }

    /// Which addresses a plugin request to `url` may reach.
    ///
    /// Public addresses always. Where the person entered the address (`entered`), their own
    /// network and this machine's loopback as well — a WebDAV server or an ntfy beside the
    /// service is an ordinary setup. Never link-local, with a cloud's metadata endpoint, nor
    /// anything else [`rd_core::address_scope`] calls local. And never the service itself: on
    /// one of its own ports loopback is refused and so is every address the service listens
    /// on, whoever entered it, and no redirect may lead to the listeners' ports ([`Self::ports`]),
    /// nor to the reverse proxy's port on this machine.
    pub(crate) fn policy(&self, entered: bool, url: &url::Url) -> rd_http::AddressPolicy {
        let policy = rd_http::AddressPolicy::new(entered)
            .refusing_redirects_to(&self.ports)
            .refusing_redirects_on_this_machine(&self.proxy_ports, self.listen);
        if self.is_own_port(url.port_or_known_default()) {
            policy.listening_on(self.listen)
        } else if entered {
            policy.with_loopback()
        } else {
            policy
        }
    }

    /// The refusal of `url` by [`Self::policy`]: a code of its own where the port is one of
    /// ours, so the person reads which rule it was — the reverse proxy's only where `url` names
    /// this machine itself, since on another host that port is anybody's.
    pub(crate) fn refused(&self, url: &url::Url) -> rd_core::Failure {
        let port = url.port_or_known_default();
        let own = port.is_some_and(|port| self.ports.contains(&port))
            || (self.is_own_port(port) && self.names_this_machine(url));
        if own {
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

impl OwnEndpoints {
    /// Whether `url`'s host is this machine by its look: a loopback literal, the listen address,
    /// or `localhost`. A name that resolves there is refused all the same, under the other code.
    fn names_this_machine(&self, url: &url::Url) -> bool {
        let address = match url.host() {
            Some(url::Host::Ipv4(address)) => std::net::IpAddr::from(address),
            Some(url::Host::Ipv6(address)) => std::net::IpAddr::from(address).to_canonical(),
            Some(url::Host::Domain(name)) => {
                let name = name.trim_end_matches('.').to_ascii_lowercase();
                return name == "localhost" || name.ends_with(".localhost");
            }
            None => return false,
        };
        address.is_loopback() || self.listen.is_some_and(|listen| listen.ip() == address)
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

/// The address rule for a request the service itself makes to an address the person entered —
/// a webhook notification target (audit 2026-10-05, S2). The rule a plugin request to an entered
/// address keeps to ([`OwnEndpoints::policy`]): their own network and this machine's loopback,
/// never link-local and never one of the service's own listeners.
#[must_use]
pub fn entered_address_policy(url: &url::Url) -> rd_http::AddressPolicy {
    current().policy(true, url)
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

    /// RD-1420-01 (audit L-10): behind a reverse proxy the external URL's port, the scheme's
    /// default where the URL names none, is ours on this machine: an entered address there is
    /// refused on loopback. On any other host it is not, and a redirect to it is followed.
    #[test]
    fn the_external_urls_port_is_ours_on_this_machine() {
        let listen = Some("127.0.0.1:8710".parse().expect("address"));
        let url = |text: &str| url::Url::parse(text).expect("url");
        let loopback = "127.0.0.1".parse().expect("loopback");
        let plain = OwnEndpoints::new(listen);
        let behind = plain
            .clone()
            .with_external_url(Some(&url("https://rd.example.com/dl")));

        for (external, port) in [
            ("https://rd.example.com/dl", 443),
            ("http://nas.lan/", 80),
            ("https://rd.example.com:8443/", 8443),
        ] {
            let own = plain.clone().with_external_url(Some(&url(external)));
            assert!(own.is_own_port(Some(port)), "{external}");
            assert_eq!(own.ports(), [8710, CLICK_N_LOAD_PORT], "{external}");
        }
        assert!(!plain.clone().with_external_url(None).is_own_port(Some(443)));

        let target = url("https://127.0.0.1/api/v1/settings");
        assert!(!behind.policy(true, &target).permits(loopback));
        assert_eq!(
            behind.refused(&target).code.as_deref(),
            Some("plugin.http_own_service")
        );
        assert_eq!(
            behind.refused(&url("https://10.0.0.5/")).code.as_deref(),
            Some("plugin.http_local_target"),
            "443 elsewhere is not ours"
        );
        assert!(
            OwnEndpoints::new(listen)
                .policy(true, &target)
                .permits(loopback)
        );
        // A hop to the proxy's port on this machine is refused; anywhere else 443 is every web
        // server's, and a hop there is followed.
        let entered = url("http://127.0.0.1:2586/alerts");
        for local in [
            "https://127.0.0.1/api/v1/settings",
            "https://localhost/api/v1/settings",
            "https://[::1]/api/v1/settings",
        ] {
            assert!(
                behind
                    .policy(true, &entered)
                    .hop_refusal(&url(local))
                    .is_some(),
                "{local}"
            );
        }
        assert_eq!(
            plain
                .clone()
                .with_external_url(None)
                .policy(true, &entered)
                .hop_refusal(&url("https://127.0.0.1/")),
            None,
            "without an external URL 443 on loopback is the person's own server"
        );
        let hop = url("https://cdn.example.com/file");
        assert_eq!(behind.policy(true, &entered).hop_refusal(&hop), None);
        assert_eq!(behind.policy(false, &hop).hop_refusal(&hop), None);
    }
}
