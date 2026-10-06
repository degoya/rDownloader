//! Host-opened sockets handed to a transfer backend as WIT resources.
//!
//! The guest never sees an address it did not already name in its manifest, never holds a
//! file descriptor, and cannot keep a socket past the call that opened it: dropping the
//! resource closes it. Reads are paced by the transfer's bandwidth limiter before the bytes
//! reach the guest, which is where a limit has to sit if it is to shape what is pulled off
//! the wire rather than only what is written to disk.

use std::{net::SocketAddr, sync::Arc};

use rd_core::{Failure, FailureKind};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{TlsConnector, client::TlsStream};

use super::state::{MAX_CONNECTIONS, SOCKET_TIMEOUT};

/// One connection the host owns on the guest's behalf.
pub(crate) struct HostConnection {
    stream: Stream,
    /// Kept so `start-tls` can upgrade in place without a second dial.
    tls: Arc<rustls::ClientConfig>,
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
    /// Between `start-tls` taking the socket and the handshake finishing.
    Upgrading,
}

impl HostConnection {
    /// Connects to exactly the addresses [`resolve_target`] checked, never to a name.
    ///
    /// A name resolved twice — once to be checked, once to be dialled — answers twice, and a
    /// server that controls it answers the second time with `127.0.0.1` (DNS rebinding,
    /// RD-191-06 PLUG-02). `host` is only the TLS server name.
    pub(crate) async fn open(
        addresses: &[SocketAddr],
        host: &str,
        tls: bool,
        config: Arc<rustls::ClientConfig>,
    ) -> Result<Self, Failure> {
        let stream = tokio::time::timeout(SOCKET_TIMEOUT, TcpStream::connect(addresses))
            .await
            .map_err(|_| transient("plugin.net_timeout", "Connecting to the server timed out"))?
            .map_err(|error| {
                transient(
                    "plugin.net_connect_failed",
                    format!("Could not connect to the server: {error}"),
                )
                .with_param("error", &error)
            })?;
        // Nagle would batch the small command lines these protocols exchange behind a delay.
        let _ = stream.set_nodelay(true);
        let mut connection = Self {
            stream: Stream::Plain(stream),
            tls: config,
        };
        if tls {
            connection.upgrade(host).await?;
        }
        Ok(connection)
    }

    pub(crate) async fn upgrade(&mut self, server_name: &str) -> Result<(), Failure> {
        let Stream::Plain(stream) = std::mem::replace(&mut self.stream, Stream::Upgrading) else {
            return Err(permanent(
                "plugin.net_already_tls",
                "This connection is already encrypted",
            ));
        };
        let name = rustls::pki_types::ServerName::try_from(server_name.to_owned())
            .map_err(|_| permanent("plugin.net_invalid_server_name", "Invalid TLS server name"))?;
        let connector = TlsConnector::from(Arc::clone(&self.tls));
        let upgraded = tokio::time::timeout(SOCKET_TIMEOUT, connector.connect(name, stream))
            .await
            .map_err(|_| transient("plugin.net_timeout", "The TLS handshake timed out"))?
            .map_err(|error| {
                // The server's own words routinely quote paths and user names, so only the
                // failure itself is reported.
                tracing::debug!(error = %error, "plugin TLS handshake failed");
                transient(
                    "plugin.net_tls_failed",
                    "The TLS handshake with the server failed",
                )
            })?;
        self.stream = Stream::Tls(Box::new(upgraded));
        Ok(())
    }

    pub(crate) async fn read(&mut self, max: usize) -> Result<Vec<u8>, Failure> {
        let mut buffer = vec![0_u8; max];
        let read = match &mut self.stream {
            Stream::Plain(stream) => timeout(stream.read(&mut buffer)).await,
            Stream::Tls(stream) => timeout(stream.read(&mut buffer)).await,
            Stream::Upgrading => {
                return Err(permanent(
                    "plugin.net_closed",
                    "This connection is not usable",
                ));
            }
        }?;
        buffer.truncate(read);
        Ok(buffer)
    }

    pub(crate) async fn write(&mut self, bytes: &[u8]) -> Result<usize, Failure> {
        match &mut self.stream {
            Stream::Plain(stream) => timeout(stream.write(bytes)).await,
            Stream::Tls(stream) => timeout(stream.write(bytes)).await,
            Stream::Upgrading => Err(permanent(
                "plugin.net_closed",
                "This connection is not usable",
            )),
        }
    }
}

async fn timeout<F>(future: F) -> Result<usize, Failure>
where
    F: std::future::Future<Output = std::io::Result<usize>>,
{
    tokio::time::timeout(SOCKET_TIMEOUT, future)
        .await
        .map_err(|_| transient("plugin.net_timeout", "The server stopped responding"))?
        .map_err(|error| {
            transient(
                "plugin.net_io_failed",
                format!("The connection failed: {error}"),
            )
            .with_param("error", &error)
        })
}

/// Resolves a connection target once and refuses addresses no transfer protocol has a reason to
/// reach.
///
/// `lookup` is called once — `FnOnce` says so — and what it answered is what
/// [`HostConnection::open`] dials, so the checked addresses and the connected one are the same
/// (PLUG-02). Loopback is where the service's own API listens and link-local carries the cloud
/// metadata endpoint; both are refused outside development mode (`allow_local`), as is
/// everything else [`rd_core::address_scope`] calls local, an IPv4 address inside an IPv6 one
/// (`::ffff:127.0.0.1`) included. Private LAN ranges are *not* refused — a file server on the
/// local network is the ordinary case for a transfer backend.
pub(crate) async fn resolve_target<F, Fut>(
    host: &str,
    port: u16,
    allow_local: bool,
    lookup: F,
) -> Result<Vec<SocketAddr>, Failure>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    let addresses = lookup(format!("{host}:{port}")).await.map_err(|error| {
        transient(
            "plugin.net_resolve_failed",
            format!("Could not resolve the server name: {error}"),
        )
        .with_param("error", &error)
    })?;
    if addresses.is_empty() {
        return Err(transient(
            "plugin.net_resolve_failed",
            "Could not resolve the server name: no address",
        )
        .with_param("error", "no address"));
    }
    if !allow_local
        && addresses
            .iter()
            .any(|address| rd_core::address_scope(address.ip()) == rd_core::AddressScope::Local)
    {
        return Err(permanent(
            "plugin.net_local_target",
            "The connection target resolves to a local address",
        ));
    }
    Ok(addresses)
}

/// Whether one more connection may be opened for this invocation.
pub(crate) fn within_connection_limit(open: usize) -> bool {
    open < MAX_CONNECTIONS
}

fn transient(code: &'static str, message: impl Into<String>) -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        code,
        message,
    )
}

fn permanent(code: &'static str, message: impl Into<String>) -> Failure {
    Failure::coded(FailureKind::Permanent, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(
        addresses: &[&str],
    ) -> impl FnOnce(String) -> std::future::Ready<std::io::Result<Vec<SocketAddr>>> {
        let addresses = addresses
            .iter()
            .map(|address| address.parse().expect("socket address"))
            .collect::<Vec<SocketAddr>>();
        move |_| std::future::ready(Ok(addresses))
    }

    #[tokio::test]
    async fn loopback_and_link_local_are_refused_but_the_lan_is_not() {
        for local in [
            "127.0.0.1:21",
            "127.13.0.9:21",
            "0.0.0.0:21",
            "169.254.169.254:21",
            "[::1]:21",
            "[fe80::1]:21",
            // An IPv4 loopback inside an IPv6 address is still loopback (PLUG-02).
            "[::ffff:127.0.0.1]:21",
            "[::ffff:169.254.169.254]:21",
        ] {
            let refused = resolve_target("files.example", 21, false, answer(&[local]))
                .await
                .expect_err("a local address is refused");
            assert_eq!(
                refused.code.as_deref(),
                Some("plugin.net_local_target"),
                "{local}"
            );
        }
        for reachable in [
            "192.168.1.10:21",
            "10.0.0.5:21",
            "172.16.4.2:21",
            "93.184.216.34:21",
        ] {
            assert!(
                resolve_target("files.example", 21, false, answer(&[reachable]))
                    .await
                    .is_ok(),
                "{reachable} is an ordinary transfer target"
            );
        }
        // One local answer among public ones is enough to refuse the name.
        assert!(
            resolve_target(
                "files.example",
                21,
                false,
                answer(&["93.184.216.34:21", "127.0.0.1:21"])
            )
            .await
            .is_err()
        );
        // Development mode reaches the local service on purpose.
        assert!(
            resolve_target("localhost", 21, true, answer(&["127.0.0.1:21"]))
                .await
                .is_ok()
        );
    }

    /// PLUG-02: a resolver that answers a public address first and loopback afterwards — what
    /// a rebinding name does — is asked once, and its first answer is what gets dialled.
    #[tokio::test]
    async fn a_rebinding_name_is_resolved_once_and_the_checked_address_is_kept() {
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let rebinding = |_target: String| {
            let call = asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let address: SocketAddr = if call == 0 {
                "93.184.216.34:21".parse().expect("public")
            } else {
                "127.0.0.1:21".parse().expect("loopback")
            };
            std::future::ready(Ok(vec![address]))
        };
        let addresses = resolve_target("rebind.example", 21, false, rebinding)
            .await
            .expect("the first answer is public");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            addresses,
            vec!["93.184.216.34:21".parse::<SocketAddr>().expect("public")]
        );
    }

    #[tokio::test]
    async fn a_name_without_an_address_is_a_transient_resolve_failure() {
        let failure = resolve_target("nowhere.example", 21, false, answer(&[]))
            .await
            .expect_err("no address");
        assert_eq!(failure.code.as_deref(), Some("plugin.net_resolve_failed"));
    }
}
