//! Whether another program holds the fixed listen port, asked before the engine binds it
//! (RD-1250-04).
//!
//! librqbit binds its TCP listener with `SO_REUSEADDR`. On Linux and macOS that still refuses a
//! port another socket listens on; on Windows it does not: the engine takes the port beside the
//! program that holds it, starts without a word, and an incoming peer lands at either of the two.
//! So before a session is built on a fixed port, the port is bound once with the standard
//! library's semantics and let go at once: on Unix with `SO_REUSEADDR`, so a closing connection's
//! `TIME_WAIT` does not count, and on Windows with no option at all, which any socket already
//! bound to the port refuses. `SO_EXCLUSIVEADDRUSE` would say the same on Windows, but setting it
//! takes an unsafe `setsockopt` (socket2 0.6 has no setter), and the plain bind needs none.
//!
//! Both wildcard addresses are tried one after the other: Windows binds `[::]` as IPv6 only, so a
//! program on `0.0.0.0` is seen by the IPv4 probe alone. uTP alone needs no probe — librqbit binds
//! its UDP socket on Windows without `SO_REUSEADDR`, so that bind refuses a held port itself.

use std::{
    io::ErrorKind,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener},
};

use crate::{TorrentService, session::SessionConfig};

/// Whether binding `port` on either wildcard address fails because a socket already has it.
/// Any other failure — IPv6 switched off, say — is the engine's own bind to report.
pub(crate) fn port_held(port: u16) -> bool {
    [
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
    ]
    .into_iter()
    .any(|address| {
        matches!(TcpListener::bind(address), Err(error) if error.kind() == ErrorKind::AddrInUse)
    })
}

/// The fixed TCP port a session for `config` would listen on, if it has one.
pub(crate) fn fixed_tcp_port(config: &SessionConfig) -> Option<u16> {
    config
        .listen_port
        .filter(|_| config.listen_mode != rd_core::TorrentListenMode::UtpOnly)
}

/// The port another program holds, among the ones a session for `config` would bind.
pub(crate) fn held_port(config: &SessionConfig) -> Option<u16> {
    fixed_tcp_port(config).filter(|port| port_held(*port))
}

impl TorrentService {
    /// The fixed listen port of the stored settings when another program holds it, so the engine
    /// cannot listen there: not started, or a rebuild refused and the previous session kept.
    /// A port the running session listens on itself is no conflict.
    pub(crate) async fn port_conflict(&self) -> Option<u16> {
        let config = SessionConfig::from_settings(&*self.inner.settings.read().await);
        let port = fixed_tcp_port(&config)?;
        let ours = self
            .inner
            .session
            .read()
            .await
            .as_ref()
            .and_then(|slot| slot.session.listen_addr())
            .map(|address| address.port());
        (ours != Some(port) && port_held(port)).then_some(port)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::{SessionConfig, fixed_tcp_port, held_port, port_held};

    fn fixed(port: u16) -> SessionConfig {
        SessionConfig {
            listen_port: Some(port),
            ..SessionConfig::default()
        }
    }

    #[test]
    fn a_port_another_socket_listens_on_is_held_and_free_again_after_it_closes() {
        let other = TcpListener::bind("0.0.0.0:0").expect("another program's socket");
        let port = other.local_addr().expect("address").port();
        assert!(port_held(port));
        assert_eq!(held_port(&fixed(port)), Some(port));
        drop(other);
        assert!(!port_held(port));
        assert_eq!(held_port(&fixed(port)), None);
    }

    #[test]
    fn only_a_fixed_port_over_tcp_is_probed() {
        assert_eq!(fixed_tcp_port(&SessionConfig::default()), None);
        assert_eq!(fixed_tcp_port(&fixed(51_413)), Some(51_413));
        let both = SessionConfig {
            listen_mode: rd_core::TorrentListenMode::TcpAndUtp,
            ..fixed(51_413)
        };
        assert_eq!(fixed_tcp_port(&both), Some(51_413));
        let utp = SessionConfig {
            listen_mode: rd_core::TorrentListenMode::UtpOnly,
            ..fixed(51_413)
        };
        assert_eq!(fixed_tcp_port(&utp), None);
    }
}
