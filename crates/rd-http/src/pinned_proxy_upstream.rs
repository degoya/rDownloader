//! The proxy a bridge (`pinned_proxy_bridge.rs`) opens its tunnels through, and the tunnel to
//! one checked address: `CONNECT <ip>:<port>` to an HTTP(S) proxy, a SOCKS5 `CONNECT` with the
//! address (RFC 1928, a login by RFC 1929) to a SOCKS5 one (RD-1320-07).
//!
//! The proxy's own name is resolved here, by the bridge, never by the client's guarded resolver:
//! that resolver judges targets alone, so no name is ever let past it because it happens to be
//! the proxy's (a stranger's `http://proxy.lan:8080/` would otherwise reach the proxy's host on
//! any port).

use std::{
    fmt, io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use anyhow::{Context, Result};
use rustls::pki_types::ServerName;
use secrecy::{ExposeSecret, SecretString};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use url::{Host, Url};
use zeroize::Zeroizing;

use crate::{HostLookup, ProxyCredentials};

/// The longest response head a proxy may answer a `CONNECT` with.
const MAX_RESPONSE_HEAD: usize = 16 * 1024;

/// The port of a SOCKS5 proxy whose address names none.
const SOCKS_PORT: u16 = 1080;

const SOCKS_VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const USER_PASS: u8 = 2;
const NO_METHOD: u8 = 0xFF;
const ATYP_IPV4: u8 = 1;
const ATYP_DOMAIN: u8 = 3;
const ATYP_IPV6: u8 = 4;
const NOT_ALLOWED: u8 = 2;

/// The status a SOCKS5 proxy's refusal is reported with, in HTTP's terms, so a caller tells
/// every refused tunnel the same way: a login it wants or refuses, a target its rules forbid, and
/// anything else it could not reach.
const SOCKS_LOGIN_REFUSED: u16 = 407;
const SOCKS_NOT_ALLOWED: u16 = 403;
const SOCKS_FAILED: u16 = 502;

/// The proxy a bridge opens its tunnels through.
pub(crate) struct Upstream {
    host: Host<String>,
    port: u16,
    protocol: Protocol,
    /// Resolves the proxy's own name: the person chose it, so the address rule does not apply,
    /// and a proxy on loopback or the person's network is fine.
    lookup: Arc<dyn HostLookup>,
}

enum Protocol {
    Http {
        /// For an `https://` proxy: the connector and the name its certificate is checked
        /// against.
        tls: Option<(tokio_rustls::TlsConnector, ServerName<'static>)>,
        /// The whole `Proxy-Authorization` value.
        authorization: Option<SecretString>,
    },
    /// `socks5://` and `socks5h://` alike: the bridge hands either the checked address.
    Socks5 {
        login: Option<(String, SecretString)>,
    },
}

impl Upstream {
    pub(crate) fn new(
        endpoint: &Url,
        credentials: Option<&ProxyCredentials>,
        custom_ca_pem: &[Vec<u8>],
        lookup: Arc<dyn HostLookup>,
    ) -> Result<Self> {
        use base64::Engine as _;

        let host = endpoint.host().context("the proxy has no host")?.to_owned();
        let (protocol, port) = match endpoint.scheme() {
            "http" | "https" => {
                let tls = if endpoint.scheme() == "https" {
                    let name = match &host {
                        Host::Domain(name) => name.clone(),
                        Host::Ipv4(address) => address.to_string(),
                        Host::Ipv6(address) => address.to_string(),
                    };
                    let name = ServerName::try_from(name).context("the proxy's name")?;
                    let config = crate::tls_client_config(custom_ca_pem)?;
                    Some((tokio_rustls::TlsConnector::from(Arc::new(config)), name))
                } else {
                    None
                };
                let authorization = credentials.map(|credentials| {
                    let pair = Zeroizing::new(format!(
                        "{}:{}",
                        credentials.username,
                        credentials.password.expose_secret()
                    ));
                    let encoded = base64::engine::general_purpose::STANDARD.encode(pair.as_bytes());
                    SecretString::from(format!("Basic {encoded}"))
                });
                let port = endpoint
                    .port_or_known_default()
                    .context("the proxy has no port")?;
                (Protocol::Http { tls, authorization }, port)
            }
            "socks5" | "socks5h" => {
                let login = credentials.map(|credentials| {
                    (credentials.username.clone(), credentials.password.clone())
                });
                let port = endpoint.port().unwrap_or(SOCKS_PORT);
                (Protocol::Socks5 { login }, port)
            }
            other => anyhow::bail!("a {other} proxy is not one a profile can name"),
        };
        Ok(Self {
            host,
            port,
            protocol,
            lookup,
        })
    }

    /// A tunnel to `target` through the proxy, and what the proxy sent beyond its answer.
    pub(crate) async fn open_tunnel(
        &self,
        target: SocketAddr,
    ) -> io::Result<(Box<dyn Io>, Vec<u8>)> {
        let tcp = self.dial().await?;
        tcp.set_nodelay(true)?;
        match &self.protocol {
            Protocol::Http { tls, authorization } => {
                let mut stream: Box<dyn Io> = match tls {
                    Some((connector, name)) => {
                        Box::new(connector.connect(name.clone(), tcp).await?)
                    }
                    None => Box::new(tcp),
                };
                let leftover = http_connect(&mut stream, target, authorization.as_ref()).await?;
                Ok((stream, leftover))
            }
            Protocol::Socks5 { login } => {
                let mut stream: Box<dyn Io> = Box::new(tcp);
                socks_connect(&mut stream, target, login.as_ref()).await?;
                Ok((stream, Vec::new()))
            }
        }
    }

    /// The first of the proxy's addresses that answers.
    async fn dial(&self) -> io::Result<TcpStream> {
        let addresses: Vec<IpAddr> = match &self.host {
            Host::Domain(name) => self.lookup.lookup(name).await?,
            Host::Ipv4(address) => vec![IpAddr::V4(*address)],
            Host::Ipv6(address) => vec![IpAddr::V6(*address)],
        };
        let mut failure = io::Error::new(io::ErrorKind::NotFound, "the proxy has no address");
        for address in addresses {
            match TcpStream::connect(SocketAddr::new(address, self.port)).await {
                Ok(stream) => return Ok(stream),
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }
}

/// A stream the bridge relays: TCP to a plain proxy, TLS to an `https://` one.
pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// The proxy refused the tunnel; the status in HTTP's terms (see [`SOCKS_FAILED`]).
#[derive(Debug)]
pub(crate) struct ProxyRefused(pub(crate) u16);

impl fmt::Display for ProxyRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the proxy refused the tunnel with status {}",
            self.0
        )
    }
}

impl std::error::Error for ProxyRefused {}

fn refused(status: u16) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, ProxyRefused(status))
}

/// `CONNECT <target>` to an HTTP proxy; returns what it sent beyond its answer.
async fn http_connect(
    stream: &mut Box<dyn Io>,
    target: SocketAddr,
    authorization: Option<&SecretString>,
) -> io::Result<Vec<u8>> {
    // `SocketAddr` writes an IPv6 address in brackets, as an authority needs it.
    let mut request = Zeroizing::new(format!(
        "CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nUser-Agent: {}\r\n",
        rd_core::user_agent!()
    ));
    if let Some(authorization) = authorization {
        request.push_str("Proxy-Authorization: ");
        request.push_str(authorization.expose_secret());
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    let (status, leftover) = read_response_head(stream).await?;
    if !(200..300).contains(&status) {
        return Err(refused(status));
    }
    Ok(leftover)
}

/// Reads a response head; returns its status and the bytes after it.
async fn read_response_head(stream: &mut Box<dyn Io>) -> io::Result<(u16, Vec<u8>)> {
    let mut head = Vec::with_capacity(512);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.extend_from_slice(&chunk[..read]);
        if let Some(end) = head.windows(4).position(|window| window == b"\r\n\r\n") {
            let leftover = head.split_off(end + 4);
            let status = std::str::from_utf8(&head)
                .ok()
                .and_then(|text| text.split_whitespace().nth(1))
                .and_then(|status| status.parse().ok())
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
            return Ok((status, leftover));
        }
        if head.len() > MAX_RESPONSE_HEAD {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
}

/// A SOCKS5 `CONNECT` to the address `target`, signed in with `login` when the profile has one.
async fn socks_connect(
    stream: &mut Box<dyn Io>,
    target: SocketAddr,
    login: Option<&(String, SecretString)>,
) -> io::Result<()> {
    let method = if login.is_some() { USER_PASS } else { NO_AUTH };
    stream.write_all(&[SOCKS_VERSION, 1, method]).await?;
    let mut chosen = [0_u8; 2];
    stream.read_exact(&mut chosen).await?;
    if chosen[0] != SOCKS_VERSION {
        return Err(io::ErrorKind::InvalidData.into());
    }
    match (chosen[1], login) {
        (NO_AUTH, _) => {}
        (USER_PASS, Some((username, password))) => {
            let password = password.expose_secret().as_bytes();
            let (Ok(user_length), Ok(password_length)) =
                (u8::try_from(username.len()), u8::try_from(password.len()))
            else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a SOCKS5 login is at most 255 bytes",
                ));
            };
            let mut request =
                Zeroizing::new(Vec::with_capacity(3 + username.len() + password.len()));
            request.extend_from_slice(&[1, user_length]);
            request.extend_from_slice(username.as_bytes());
            request.push(password_length);
            request.extend_from_slice(password);
            stream.write_all(&request).await?;
            let mut answer = [0_u8; 2];
            stream.read_exact(&mut answer).await?;
            if answer[1] != 0 {
                return Err(refused(SOCKS_LOGIN_REFUSED));
            }
        }
        // It wants a login this profile does not have, or none it offers.
        (NO_METHOD | USER_PASS, _) => return Err(refused(SOCKS_LOGIN_REFUSED)),
        _ => return Err(io::ErrorKind::InvalidData.into()),
    }

    let mut request = vec![SOCKS_VERSION, 1, 0];
    match target.ip() {
        IpAddr::V4(address) => {
            request.push(ATYP_IPV4);
            request.extend_from_slice(&address.octets());
        }
        IpAddr::V6(address) => {
            request.push(ATYP_IPV6);
            request.extend_from_slice(&address.octets());
        }
    }
    request.extend_from_slice(&target.port().to_be_bytes());
    stream.write_all(&request).await?;

    let mut reply = [0_u8; 4];
    stream.read_exact(&mut reply).await?;
    let bound = match reply[3] {
        ATYP_IPV4 => 4,
        ATYP_IPV6 => 16,
        ATYP_DOMAIN => usize::from(stream.read_u8().await?),
        _ => return Err(io::ErrorKind::InvalidData.into()),
    };
    // The bound address and port, which the bridge has no use for.
    let mut rest = vec![0_u8; bound + 2];
    stream.read_exact(&mut rest).await?;
    match reply[1] {
        0 => Ok(()),
        NOT_ALLOWED => Err(refused(SOCKS_NOT_ALLOWED)),
        _ => Err(refused(SOCKS_FAILED)),
    }
}
