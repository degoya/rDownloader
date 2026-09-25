//! Host-opened sockets handed to a transfer backend as WIT resources.
//!
//! The guest never sees an address it did not already name in its manifest, never holds a
//! file descriptor, and cannot keep a socket past the call that opened it: dropping the
//! resource closes it. Reads are paced by the transfer's bandwidth limiter before the bytes
//! reach the guest, which is where a limit has to sit if it is to shape what is pulled off
//! the wire rather than only what is written to disk.

use std::{net::IpAddr, sync::Arc};

use rd_core::{Failure, FailureKind};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{TlsConnector, client::TlsStream};

use super::state::{MAX_CONNECTIONS, SOCKET_TIMEOUT};

/// One connection the host owns on the guest's behalf.
pub struct HostConnection {
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
    pub(crate) async fn open(
        host: &str,
        port: u16,
        tls: bool,
        config: Arc<rustls::ClientConfig>,
    ) -> Result<Self, Failure> {
        let stream = tokio::time::timeout(SOCKET_TIMEOUT, TcpStream::connect((host, port)))
            .await
            .map_err(|_| transient("plugin.net_timeout", "Connecting to the server timed out"))?
            .map_err(|error| {
                transient(
                    "plugin.net_connect_failed",
                    format!("Could not connect to the server: {error}"),
                )
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
        })
}

/// Whether a resolved address is one no transfer protocol has a reason to reach.
///
/// Loopback is where the service's own API listens; link-local carries the cloud metadata
/// endpoint. A signed plugin naming either in its manifest would still be asking for
/// something it cannot legitimately need, so both are refused outside development mode.
/// Private LAN ranges are *not* refused — a file server on the local network is the ordinary
/// case for a transfer backend.
pub(crate) fn is_local_only(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
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

    #[test]
    fn loopback_and_link_local_are_refused_but_the_lan_is_not() {
        for local in [
            "127.0.0.1",
            "127.13.0.9",
            "0.0.0.0",
            "169.254.169.254",
            "::1",
            "fe80::1",
        ] {
            assert!(
                is_local_only(local.parse().expect("address")),
                "{local} must not be reachable"
            );
        }
        for reachable in ["192.168.1.10", "10.0.0.5", "172.16.4.2", "93.184.216.34"] {
            assert!(
                !is_local_only(reachable.parse().expect("address")),
                "{reachable} is an ordinary transfer target"
            );
        }
    }
}
