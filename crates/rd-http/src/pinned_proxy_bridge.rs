//! The bridge behind [`super::with_proxy`]: a SOCKS5 listener on loopback that reqwest hands
//! each target address it resolved and checked, and that asks the profile's proxy — HTTP(S) or
//! SOCKS5 — for a tunnel to exactly that address (`pinned_proxy_upstream.rs`, RD-1320-07).

use std::{
    fmt, io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use reqwest::Proxy;
use subtle::ConstantTimeEq as _;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    task::AbortHandle,
};
use url::Url;
use zeroize::Zeroizing;

use super::upstream::{Io, ProxyRefused, Upstream};
use crate::{AddressPolicy, tunnel_refusal::Tickets};

/// SOCKS5 (RFC 1928, RFC 1929) as far as the bridge speaks it.
const SOCKS_VERSION: u8 = 5;
const USER_PASS: u8 = 2;
const NO_METHOD: u8 = 0xFF;
const CONNECT: u8 = 1;
const ATYP_IPV4: u8 = 1;
const ATYP_IPV6: u8 = 4;
const SUCCEEDED: u8 = 0;
const GENERAL_FAILURE: u8 = 1;
const NOT_ALLOWED: u8 = 2;
const CONNECTION_REFUSED: u8 = 5;
const COMMAND_NOT_SUPPORTED: u8 = 7;
const ADDRESS_TYPE_NOT_SUPPORTED: u8 = 8;

/// What every connection to a bridge shares.
struct Shared {
    /// The SOCKS5 password only the bridge's own client knows: anything else on this machine
    /// can reach a loopback port, and would otherwise tunnel through the person's proxy, with
    /// their credentials.
    token: String,
    policy: AddressPolicy,
    upstream: Upstream,
    /// Where a refused tunnel's status is filed for the client's connector layer.
    tickets: Arc<Tickets>,
}

/// A loopback SOCKS5 listener that turns each address its client resolved and checked into a
/// `CONNECT` to the upstream proxy. It stops when the client that holds it is dropped.
pub(crate) struct Bridge {
    url: Url,
    listener: AbortHandle,
}

impl Bridge {
    /// Opens the listener. Needs the runtime the client will run on.
    pub(crate) fn start(
        upstream: Upstream,
        policy: AddressPolicy,
        tickets: Arc<Tickets>,
    ) -> Result<Self> {
        let runtime = tokio::runtime::Handle::try_current()
            .context("a guarded client behind a proxy is built inside the runtime")?;
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .context("open the proxy bridge on loopback")?;
        listener
            .set_nonblocking(true)
            .context("open the proxy bridge on loopback")?;
        let port = listener.local_addr().context("the bridge's port")?.port();
        let token = bridge_token()?;
        let url = Url::parse(&format!("socks5://rd:{token}@127.0.0.1:{port}"))
            .context("the bridge's address")?;
        let shared = Arc::new(Shared {
            token,
            policy,
            upstream,
            tickets,
        });
        let listener = runtime.spawn(accept(listener, shared)).abort_handle();
        Ok(Self { url, listener })
    }

    /// The bridge as the client's proxy. The proxy owns it, so the listener lives exactly as
    /// long as the client does. reqwest asks for the proxy inside each connect call, so the
    /// SOCKS user name carries that call's ticket (`tunnel_refusal.rs`).
    pub(crate) fn into_proxy(self) -> Proxy {
        // The whole bridge moves into the closure, not just its address (`Bridge` is `Drop`).
        Proxy::custom(move |_| {
            let bridge = &self;
            let mut url = bridge.url.clone();
            if let Some(ticket) = crate::tunnel_refusal::current_ticket() {
                let _ = url.set_username(&ticket.to_string());
            }
            Some(url)
        })
    }

    #[cfg(test)]
    pub(crate) fn url(&self) -> &Url {
        &self.url
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.listener.abort();
    }
}

/// 32 random bytes in hex, from the TLS stack's random source.
fn bridge_token() -> Result<String> {
    use fmt::Write as _;

    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    let mut bytes = Zeroizing::new([0_u8; 32]);
    provider
        .secure_random
        .fill(&mut bytes[..])
        .map_err(|_| anyhow::anyhow!("no secure random source for the proxy bridge"))?;
    let mut token = String::with_capacity(64);
    for byte in bytes.iter() {
        let _ = write!(token, "{byte:02x}");
    }
    Ok(token)
}

async fn accept(listener: std::net::TcpListener, shared: Arc<Shared>) {
    let listener = match tokio::net::TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%error, "the proxy bridge could not listen");
            return;
        }
    };
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(serve(stream, Arc::clone(&shared)));
            }
            Err(error) => {
                // Out of descriptors, most likely; a pause instead of a busy loop.
                tracing::debug!(%error, "the proxy bridge could not accept");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// One connection: the SOCKS5 handshake, the tunnel, then bytes both ways.
async fn serve(mut client: TcpStream, shared: Arc<Shared>) {
    let opened = tokio::time::timeout(crate::CLIENT_CONNECT_TIMEOUT, open(&mut client, &shared));
    let (mut upstream, leftover) = match opened.await {
        Ok(Ok(Some(tunnel))) => tunnel,
        Ok(Ok(None)) | Err(_) => return,
        Ok(Err(error)) => {
            tracing::debug!(%error, "a proxy bridge handshake failed");
            return;
        }
    };
    if reply(&mut client, SUCCEEDED).await.is_err() || client.write_all(&leftover).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

/// The handshake and the tunnel; `None` when the client was refused, and told so.
async fn open(
    client: &mut TcpStream,
    shared: &Shared,
) -> io::Result<Option<(Box<dyn Io>, Vec<u8>)>> {
    let Some((target, ticket)) = admit(client, &shared.token).await? else {
        return Ok(None);
    };
    // The resolver already checked it; the bridge relays nothing else, whoever asks.
    if !shared.policy.permits(target.ip()) {
        reply(client, NOT_ALLOWED).await?;
        return Ok(None);
    }
    match shared.upstream.open_tunnel(target).await {
        Ok(tunnel) => Ok(Some(tunnel)),
        Err(error) => {
            tracing::warn!(%error, "the proxy opened no tunnel to the checked address");
            let status = error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<ProxyRefused>())
                .map(|refused| refused.0);
            if let (Some(status), Some(ticket)) = (status, ticket) {
                shared.tickets.refuse(ticket, status);
            }
            let code = if status.is_some() {
                CONNECTION_REFUSED
            } else {
                GENERAL_FAILURE
            };
            reply(client, code).await?;
            Ok(None)
        }
    }
}

/// The SOCKS5 handshake: this bridge's password, then a `CONNECT` to an address. A name is
/// refused — the client resolves every target itself, so a name here means it did not.
/// Returns the address and the connect call's ticket, the user name.
async fn admit(
    client: &mut TcpStream,
    token: &str,
) -> io::Result<Option<(SocketAddr, Option<u64>)>> {
    let mut greeting = [0_u8; 2];
    client.read_exact(&mut greeting).await?;
    if greeting[0] != SOCKS_VERSION {
        return Ok(None);
    }
    let mut methods = vec![0_u8; usize::from(greeting[1])];
    client.read_exact(&mut methods).await?;
    if !methods.contains(&USER_PASS) {
        client.write_all(&[SOCKS_VERSION, NO_METHOD]).await?;
        return Ok(None);
    }
    client.write_all(&[SOCKS_VERSION, USER_PASS]).await?;

    let mut version_and_length = [0_u8; 2];
    client.read_exact(&mut version_and_length).await?;
    let mut username = vec![0_u8; usize::from(version_and_length[1])];
    client.read_exact(&mut username).await?;
    let mut password = Zeroizing::new(vec![0_u8; usize::from(client.read_u8().await?)]);
    client.read_exact(&mut password).await?;
    if !bool::from(password.as_slice().ct_eq(token.as_bytes())) {
        client.write_all(&[1, 1]).await?;
        return Ok(None);
    }
    client.write_all(&[1, 0]).await?;

    let mut request = [0_u8; 4];
    client.read_exact(&mut request).await?;
    let address = match request[3] {
        ATYP_IPV4 => {
            let mut octets = [0_u8; 4];
            client.read_exact(&mut octets).await?;
            IpAddr::from(Ipv4Addr::from(octets))
        }
        ATYP_IPV6 => {
            let mut octets = [0_u8; 16];
            client.read_exact(&mut octets).await?;
            IpAddr::from(Ipv6Addr::from(octets))
        }
        _ => {
            reply(client, ADDRESS_TYPE_NOT_SUPPORTED).await?;
            return Ok(None);
        }
    };
    let port = client.read_u16().await?;
    if request[0] != SOCKS_VERSION || request[1] != CONNECT {
        reply(client, COMMAND_NOT_SUPPORTED).await?;
        return Ok(None);
    }
    let ticket = std::str::from_utf8(&username)
        .ok()
        .and_then(|name| name.parse().ok());
    Ok(Some((SocketAddr::new(address, port), ticket)))
}

/// A SOCKS5 reply without a bound address.
async fn reply(client: &mut TcpStream, code: u8) -> io::Result<()> {
    client
        .write_all(&[SOCKS_VERSION, code, 0, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
        .await
}
