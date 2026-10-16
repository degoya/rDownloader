//! What the pinned-proxy tests talk to besides [`crate::test_support`]: a client over a chosen
//! lookup, a plain origin that records what reached it, and a SOCKS5 proxy that records which
//! target it was asked for and connects it to the loopback port asked for.

use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use url::Url;

pub(super) use crate::test_support::{
    Answers, Log, PUBLIC, TestCa, connect_proxy, entries, refusing_proxy, tls_origin,
};
use crate::{AddressPolicy, ProxyCredentials, test_support::listen};

/// A client under the public rule behind `endpoint`, trusting `ca` for its targets and
/// `proxy_ca` for the proxy itself.
pub(super) fn client(
    endpoint: &str,
    credentials: Option<&ProxyCredentials>,
    lookup: Arc<Answers>,
    ca: Option<&str>,
    proxy_ca: &[Vec<u8>],
) -> reqwest::Client {
    let mut builder =
        crate::client_builder(crate::ClientPurpose::Direct).connect_timeout(Duration::from_secs(5));
    if let Some(pem) = ca {
        let certificate = reqwest::Certificate::from_pem(pem.as_bytes()).expect("test CA");
        builder = builder.add_root_certificate(certificate);
    }
    let endpoint = Url::parse(endpoint).expect("endpoint");
    super::pinned(
        builder,
        &AddressPolicy::new(false),
        &endpoint,
        credentials,
        proxy_ca,
        lookup,
    )
    .expect("builder")
    .build()
    .expect("client")
}

pub(super) async fn fetch(client: &reqwest::Client, url: &str) -> String {
    client
        .get(url)
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body")
}

/// A plain HTTP origin that answers `ok` and records each request as
/// `<method> <target> host=<Host> proxy-authorization=<value or ->`.
pub(super) async fn plain_origin() -> (rd_testkit::TestServer, Log) {
    let log = Log::default();
    let seen = Arc::clone(&log);
    let app = axum::Router::new().fallback(move |request: axum::extract::Request| {
        let seen = Arc::clone(&seen);
        async move {
            let header = |name: axum::http::HeaderName| {
                request
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("-")
                    .to_owned()
            };
            let entry = format!(
                "{} {} host={} proxy-authorization={}",
                request.method(),
                request.uri(),
                header(axum::http::header::HOST),
                header(axum::http::header::PROXY_AUTHORIZATION),
            );
            rd_testkit::lock(&seen).push(entry);
            "ok"
        }
    });
    (rd_testkit::serve(app).await, log)
}

/// A SOCKS5 proxy without authentication that records each target as `<address>:<port>` or
/// `<name>:<port>`, and connects it to the loopback port asked for.
pub(super) async fn socks_proxy() -> (u16, Log) {
    socks_proxy_with(None).await
}

/// [`socks_proxy`] that wants the login `user`, `password` (RFC 1929) and records a signed-in
/// target as `<user>@<address>:<port>`.
pub(super) async fn socks_proxy_with(login: Option<(&'static str, &'static str)>) -> (u16, Log) {
    let (listener, port) = listen().await;
    let log = Log::default();
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(socks_tunnel(stream, Arc::clone(&seen), login));
        }
    });
    (port, log)
}

async fn socks_tunnel(
    mut stream: TcpStream,
    seen: Log,
    login: Option<(&'static str, &'static str)>,
) -> std::io::Result<()> {
    let mut greeting = [0_u8; 2];
    stream.read_exact(&mut greeting).await?;
    let mut methods = vec![0_u8; usize::from(greeting[1])];
    stream.read_exact(&mut methods).await?;
    let user = match login {
        None => {
            stream.write_all(&[5, 0]).await?;
            None
        }
        Some(_) if !methods.contains(&2) => {
            stream.write_all(&[5, 0xFF]).await?;
            return Ok(());
        }
        Some((user, password)) => {
            stream.write_all(&[5, 2]).await?;
            let _version = stream.read_u8().await?;
            let mut name = vec![0_u8; usize::from(stream.read_u8().await?)];
            stream.read_exact(&mut name).await?;
            let mut given = vec![0_u8; usize::from(stream.read_u8().await?)];
            stream.read_exact(&mut given).await?;
            if name != user.as_bytes() || given != password.as_bytes() {
                stream.write_all(&[1, 1]).await?;
                return Ok(());
            }
            stream.write_all(&[1, 0]).await?;
            Some(user)
        }
    };
    let mut request = [0_u8; 4];
    stream.read_exact(&mut request).await?;
    let host = match request[3] {
        1 => {
            let mut octets = [0_u8; 4];
            stream.read_exact(&mut octets).await?;
            Ipv4Addr::from(octets).to_string()
        }
        3 => {
            let mut name = vec![0_u8; usize::from(stream.read_u8().await?)];
            stream.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        _ => return Ok(()),
    };
    let port = stream.read_u16().await?;
    let entry = match user {
        Some(user) => format!("{user}@{host}:{port}"),
        None => format!("{host}:{port}"),
    };
    rd_testkit::lock(&seen).push(entry);
    let mut origin = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
    stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    tokio::io::copy_bidirectional(&mut stream, &mut origin).await?;
    Ok(())
}
