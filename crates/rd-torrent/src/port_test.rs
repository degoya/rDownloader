//! Whether the incoming peer port works, judged from this machine alone (RD-1240-16).
//!
//! No outside service is asked: one would learn the user's address and port and that they run
//! BitTorrent. What the engine itself can show instead is two things. The listener answers a
//! local TCP connection, which proves the port is bound but not that the internet reaches it;
//! and a peer that connected *in* — librqbit counts incoming connections per peer — proves it
//! does. Without such a peer the verdict stays "listening": a port behind a closed router or a
//! VPN without forwarding looks the same as an open one nobody has dialled yet.

use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{TorrentService, session::SessionConfig};

/// How long the local connection to the listener may take.
const LOCAL_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// What the port test concluded.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TorrentPortVerdict {
    /// A peer connected in: the port is reachable from outside.
    Reachable,
    /// The listener works, but no peer has connected in yet, so reachability is unproven.
    Listening,
    /// The engine has no listener, or it did not answer a local connection.
    NotListening,
    /// The torrent session could not be started; `error` says why.
    Unavailable,
}

/// The result of one port test.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct TorrentPortTest {
    pub verdict: TorrentPortVerdict,
    /// The port the engine listens on, when it has a listener.
    pub listen_port: Option<u16>,
    /// The port announced to trackers: the configured announce port, else the listen port.
    pub announce_port: Option<u16>,
    /// Whether the listener answered a local TCP connection; `None` when it was not tried,
    /// because the engine accepts uTP only or is bound to one network interface.
    pub local_tcp_accepted: Option<bool>,
    /// Torrents running in the engine right now, whose peers were looked at.
    pub live_torrents: u32,
    /// Connected peers that reached this machine through the port.
    pub incoming_peers: u32,
    /// Whether UPnP port forwarding was requested.
    pub upnp_enabled: bool,
    /// Whether outgoing peer connections go through a SOCKS5 proxy; incoming ones never do.
    pub peer_proxy_configured: bool,
    /// Why the session could not be started, for `unavailable`.
    pub error: Option<String>,
    pub tested_at: chrono::DateTime<chrono::Utc>,
}

/// The verdict from what the test found.
fn verdict(
    listening: bool,
    local_tcp_accepted: Option<bool>,
    incoming_peers: u32,
) -> TorrentPortVerdict {
    if incoming_peers > 0 {
        TorrentPortVerdict::Reachable
    } else if !listening || local_tcp_accepted == Some(false) {
        TorrentPortVerdict::NotListening
    } else {
        TorrentPortVerdict::Listening
    }
}

/// The addresses a local connection to a listener on `listen` goes to.
fn local_targets(listen: SocketAddr) -> Vec<SocketAddr> {
    if !listen.ip().is_unspecified() {
        return vec![listen];
    }
    // A dual-stack listener takes IPv4 too; IPv6 loopback is the fallback where IPv4 is not
    // mapped onto it.
    let mut targets = vec![SocketAddr::from((Ipv4Addr::LOCALHOST, listen.port()))];
    if listen.is_ipv6() {
        targets.push(SocketAddr::from((Ipv6Addr::LOCALHOST, listen.port())));
    }
    targets
}

/// Whether the listener accepts a TCP connection from this machine.
async fn accepts_locally(listen: SocketAddr) -> bool {
    for target in local_targets(listen) {
        let attempt =
            tokio::time::timeout(LOCAL_CHECK_TIMEOUT, tokio::net::TcpStream::connect(target)).await;
        if matches!(attempt, Ok(Ok(_))) {
            return true;
        }
    }
    false
}

/// Whether a local TCP connection says anything about this listener.
///
/// Not for uTP alone, which has no TCP listener, and not with an interface binding: a socket
/// bound to a VPN interface never sees a connection that arrives on loopback.
fn tcp_probe_applies(config: &SessionConfig) -> bool {
    config.listen_mode != rd_core::TorrentListenMode::UtpOnly && config.bind_interface.is_none()
}

impl TorrentService {
    /// Tests the incoming peer port; starts the session when it is not running yet.
    pub async fn port_test(&self) -> TorrentPortTest {
        let settings = self.inner.settings.read().await.clone();
        let mut report = TorrentPortTest {
            verdict: TorrentPortVerdict::Unavailable,
            listen_port: None,
            announce_port: None,
            local_tcp_accepted: None,
            live_torrents: 0,
            incoming_peers: 0,
            upnp_enabled: settings.torrent_upnp_enabled,
            peer_proxy_configured: settings.torrent_proxy_profile_id.is_some(),
            error: None,
            tested_at: chrono::Utc::now(),
        };
        if let Err(error) = self.session().await {
            report.error = Some(format!("{error:#}"));
            return report;
        }
        let Some((session, config)) = self
            .inner
            .session
            .read()
            .await
            .as_ref()
            .map(|slot| (slot.session.clone(), slot.config.clone()))
        else {
            report.error = Some("the torrent session is not running".to_owned());
            return report;
        };
        let listen = session.listen_addr();
        report.listen_port = listen.map(|address| address.port());
        report.announce_port = session.announce_port();
        if let Some(listen) = listen
            && tcp_probe_applies(&config)
        {
            report.local_tcp_accepted = Some(accepts_locally(listen).await);
        }
        let api = librqbit::api::Api::new(session, None);
        let registered = self.inner.registry.read().await.snapshot();
        for (_, entry) in registered {
            // A torrent that is not live has no peers to count.
            let Ok(peers) = api.api_peer_stats(entry.handle(), Default::default()) else {
                continue;
            };
            report.live_torrents += 1;
            let incoming = peers
                .peers
                .values()
                .filter(|peer| peer.counters.incoming_connections > 0)
                .count();
            report.incoming_peers = report
                .incoming_peers
                .saturating_add(u32::try_from(incoming).unwrap_or(u32::MAX));
        }
        report.verdict = verdict(
            listen.is_some(),
            report.local_tcp_accepted,
            report.incoming_peers,
        );
        report
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::{
        SessionConfig, TorrentPortVerdict, accepts_locally, local_targets, tcp_probe_applies,
        verdict,
    };

    #[test]
    fn an_incoming_peer_is_the_only_proof_of_reachability() {
        assert_eq!(verdict(true, Some(true), 1), TorrentPortVerdict::Reachable);
        assert_eq!(verdict(true, None, 2), TorrentPortVerdict::Reachable);
        assert_eq!(verdict(true, Some(true), 0), TorrentPortVerdict::Listening);
        assert_eq!(verdict(true, None, 0), TorrentPortVerdict::Listening);
        assert_eq!(
            verdict(true, Some(false), 0),
            TorrentPortVerdict::NotListening
        );
        assert_eq!(verdict(false, None, 0), TorrentPortVerdict::NotListening);
    }

    #[test]
    fn an_unspecified_listener_is_dialled_on_loopback() {
        let any_v6: SocketAddr = "[::]:6881".parse().expect("address");
        let targets: Vec<String> = local_targets(any_v6)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(targets, ["127.0.0.1:6881", "[::1]:6881"]);
        let any_v4: SocketAddr = "0.0.0.0:6881".parse().expect("address");
        assert_eq!(local_targets(any_v4).len(), 1);
        let bound: SocketAddr = "192.0.2.4:6881".parse().expect("address");
        assert_eq!(local_targets(bound), [bound]);
    }

    #[test]
    fn utp_only_and_a_bound_interface_skip_the_local_probe() {
        let plain = SessionConfig::default();
        assert!(tcp_probe_applies(&plain));
        let utp = SessionConfig {
            listen_mode: rd_core::TorrentListenMode::UtpOnly,
            ..SessionConfig::default()
        };
        assert!(!tcp_probe_applies(&utp));
        let bound = SessionConfig {
            bind_interface: Some("wg0".to_owned()),
            ..SessionConfig::default()
        };
        assert!(!tcp_probe_applies(&bound));
    }

    /// A service whose session listens as `listen` says and never reaches the network.
    async fn offline_service(
        scratch: &std::path::Path,
        listen: Option<librqbit::ListenerOptions>,
    ) -> crate::TorrentService {
        let database = rd_db::Database::open(scratch.join("torrent.sqlite3"))
            .await
            .expect("database");
        let settings = crate::shared_settings(&database).await.expect("settings");
        let output = scratch.join("downloads");
        let service =
            crate::TorrentService::start(database, settings, scratch.to_owned(), output.clone());
        let session = librqbit::Session::new_with_opts(
            output,
            librqbit::SessionOptions {
                dht: None,
                listen,
                disable_trackers: true,
                disable_local_service_discovery: true,
                ..Default::default()
            },
        )
        .await
        .expect("session");
        *service.inner.session.write().await = Some(crate::session::SessionSlot {
            session,
            config: SessionConfig::default(),
            generation: 1,
        });
        service
    }

    fn scratch() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rd-torrent-port-test-{}",
            rd_core::DownloadId::new()
        ));
        std::fs::create_dir_all(&path).expect("scratch dir");
        path
    }

    #[tokio::test]
    async fn a_listening_engine_without_incoming_peers_is_listening_not_reachable() {
        let directory = scratch();
        let listen = librqbit::ListenerOptions {
            listen_addr: "127.0.0.1:0".parse().expect("address"),
            mode: librqbit::ListenerMode::TcpOnly,
            ipv4_only: true,
            ..Default::default()
        };
        let service = offline_service(&directory, Some(listen)).await;
        let report = service.port_test().await;
        service.shutdown();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(report.verdict, TorrentPortVerdict::Listening, "{report:?}");
        assert!(report.listen_port.is_some_and(|port| port != 0));
        assert_eq!(report.local_tcp_accepted, Some(true));
        assert_eq!(report.live_torrents, 0);
        assert_eq!(report.incoming_peers, 0);
        assert_eq!(report.error, None);
    }

    #[tokio::test]
    async fn an_engine_without_a_listener_is_not_listening() {
        let directory = scratch();
        let service = offline_service(&directory, None).await;
        let report = service.port_test().await;
        service.shutdown();
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(report.verdict, TorrentPortVerdict::NotListening);
        assert_eq!(report.listen_port, None);
        assert_eq!(report.local_tcp_accepted, None);
    }

    #[tokio::test]
    async fn the_local_probe_tells_an_open_port_from_a_closed_one() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let open = listener.local_addr().expect("address");
        assert!(accepts_locally(open).await);
        drop(listener);
        assert!(!accepts_locally(open).await);
    }
}
