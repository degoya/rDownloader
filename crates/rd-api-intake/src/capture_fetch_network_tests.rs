//! The hand-over fetch goes out on the service's own network (RD-1320-06): through the proxy,
//! never around one it cannot use, and trusting the custom CA. Behind a proxy the checked
//! address is pinned unless the profile says the proxy resolves names itself (RD-1320-07).

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::{FetchNetwork, FetchPlan, fetch_once};

const BODY: &[u8] = b"the handed-over file";

/// The request line and `Cookie` header of every request a listener saw.
type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// Reads one request head and answers it with [`BODY`], or for a path that ends in
/// `/redirect-inside` with a redirect to this machine.
async fn answer<S>(mut stream: S, log: &Log)
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let line = text.lines().next().unwrap_or_default().to_owned();
    let cookie = text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("cookie")
            .then(|| value.trim().to_owned())
    });
    let inside = line.contains("/redirect-inside ");
    rd_testkit::lock(log).push((line, cookie));
    if inside {
        let redirect = "HTTP/1.1 302 Found\r\nlocation: http://localhost:9/admin\r\n\
                        content-length: 0\r\nconnection: close\r\n\r\n";
        let _ = stream.write_all(redirect.as_bytes()).await;
        let _ = stream.shutdown().await;
        return;
    }
    let mut response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n",
        BODY.len()
    )
    .into_bytes();
    response.extend_from_slice(BODY);
    let _ = stream.write_all(&response).await;
    let _ = stream.shutdown().await;
}

/// A plain HTTP listener — a proxy or a server, both answer the same way.
async fn plain() -> (SocketAddr, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let log = Log::default();
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move { answer(stream, &seen).await });
        }
    });
    (address, log)
}

/// An HTTPS listener on `127.0.0.1` whose certificate a CA of its own signed; the CA's PEM.
async fn tls() -> (SocketAddr, Vec<u8>) {
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "rDownloader test CA");
    let ca_key = rcgen::KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca cert");
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let mut server_params = rcgen::CertificateParams::new(Vec::new()).expect("server params");
    server_params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(std::net::IpAddr::from([
            127, 0, 0, 1,
        ])));
    let server_key = rcgen::KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .expect("server cert");
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(server_cert.der().to_vec())],
            PrivateKeyDer::try_from(server_key.serialize_der()).expect("server key der"),
        )
        .expect("server config");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let log = Log::default();
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                if let Ok(stream) = acceptor.accept(stream).await {
                    answer(stream, &log).await;
                }
            });
        }
    });
    (address, ca_cert.pem().into_bytes())
}

fn plan(url: &str, cookies: Option<&str>) -> FetchPlan {
    FetchPlan::new(url, cookies, None, None).expect("plan")
}

/// The proxy at `address`, resolving names itself: the behaviour before RD-1320-07, which the
/// fixtures below, that play proxy and server at once for names nothing resolves, rely on.
fn proxy(address: SocketAddr) -> rd_scheduler::ToolProxy {
    pinned_proxy(address).resolving_names(true)
}

/// The proxy at `address` as a profile has it by default: handed the checked address.
fn pinned_proxy(address: SocketAddr) -> rd_scheduler::ToolProxy {
    let endpoint = format!("http://{address}").parse().expect("endpoint");
    rd_scheduler::ToolProxy::new(rd_core::ProxyKind::Http, endpoint, None, None).expect("proxy")
}

/// A proxy that refuses every request with `403` and records each request line.
async fn refusing() -> (SocketAddr, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let log = Log::default();
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => raw.extend_from_slice(&buffer[..read]),
                }
            }
            let text = String::from_utf8_lossy(&raw).into_owned();
            let line = text.lines().next().unwrap_or_default().to_owned();
            rd_testkit::lock(&seen).push((line, None));
            let _ = stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n")
                .await;
        }
    });
    (address, log)
}

/// Through the proxy, with the cookies. The target's name does not resolve, so a request the
/// proxy saw went through it; and the guard, which judges the target, lets a name it cannot
/// resolve pass while the proxy on loopback is not its business.
#[tokio::test]
async fn the_fetch_goes_through_the_proxy_with_the_cookies() {
    let (address, seen) = plain().await;
    let guard = rd_http::AddressPolicy::new(true);
    let network = FetchNetwork::new(Some(proxy(address)), Vec::new());

    let fetched = fetch_once(
        &plan("http://indexer.test/getnzb/abc", Some("uid=7; sess=s3cr3t")),
        1024,
        Some(&guard),
        &network,
    )
    .await
    .expect("fetched");

    assert_eq!(fetched.bytes, BODY);
    let seen = rd_testkit::lock(&seen).clone();
    assert_eq!(
        seen,
        [(
            "GET http://indexer.test/getnzb/abc HTTP/1.1".to_owned(),
            Some("uid=7; sess=s3cr3t".to_owned())
        )]
    );
}

/// A proxy that does not answer is an error; the address is not fetched directly instead.
#[tokio::test]
async fn an_unreachable_proxy_is_an_error_and_no_direct_fetch() {
    let (server, seen) = plain().await;
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        listener.local_addr().expect("address")
    };
    let network = FetchNetwork::new(Some(proxy(closed)), Vec::new());

    let error = fetch_once(
        &plan(&format!("http://{server}/file.nzb"), None),
        1024,
        None,
        &network,
    )
    .await
    .expect_err("no proxy, no fetch");

    assert_eq!(error.code(), "capture.fetch_failed");
    assert!(rd_testkit::lock(&seen).is_empty(), "fetched directly");
}

/// Without a proxy the fetch goes direct, as before.
#[tokio::test]
async fn without_a_proxy_the_fetch_goes_direct() {
    let (server, seen) = plain().await;
    let fetched = fetch_once(
        &plan(&format!("http://{server}/file.nzb"), None),
        1024,
        None,
        &FetchNetwork::default(),
    )
    .await
    .expect("fetched");
    assert_eq!(fetched.bytes, BODY);
    assert_eq!(rd_testkit::lock(&seen).len(), 1);
}

/// A server whose certificate only the custom CA vouches for is fetched with it, and refused
/// without it.
#[tokio::test]
async fn the_custom_ca_is_trusted() {
    let (address, ca_pem) = tls().await;
    let url = format!("https://{address}/file.nzb");

    let refused = fetch_once(&plan(&url, None), 1024, None, &FetchNetwork::default()).await;
    assert_eq!(
        refused.expect_err("untrusted").code(),
        "capture.fetch_failed"
    );

    let fetched = fetch_once(
        &plan(&url, None),
        1024,
        None,
        &FetchNetwork::new(None, vec![ca_pem]),
    )
    .await
    .expect("trusted");
    assert_eq!(fetched.bytes, BODY);
}

/// Through a proxy the target is still judged before the request: a name that resolves to this
/// machine is refused, and the proxy is never asked.
#[tokio::test]
async fn through_the_proxy_a_name_that_resolves_inside_is_refused() {
    let (address, seen) = plain().await;
    let guard = rd_http::AddressPolicy::new(true);
    let network = FetchNetwork::new(Some(proxy(address)), Vec::new());

    for target in [
        "http://localhost:8710/api/v1/settings",
        "http://127.0.0.1:8710/",
    ] {
        let error = fetch_once(&plan(target, None), 1024, Some(&guard), &network)
            .await
            .expect_err(target);
        assert_eq!(error.code(), "capture.fetch_address_refused", "{target}");
    }
    assert!(rd_testkit::lock(&seen).is_empty(), "the proxy was asked");
}

/// And every hop is: a proxied answer that redirects to this machine is not followed.
#[tokio::test]
async fn through_the_proxy_a_redirect_inside_is_refused() {
    let (address, seen) = plain().await;
    let guard = rd_http::AddressPolicy::new(true);
    let network = FetchNetwork::new(Some(proxy(address)), Vec::new());

    let error = fetch_once(
        &plan("http://indexer.test/redirect-inside", None),
        1024,
        Some(&guard),
        &network,
    )
    .await
    .expect_err("refused");

    assert_eq!(error.code(), "capture.fetch_address_refused");
    let seen = rd_testkit::lock(&seen).clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        seen[0].0,
        "GET http://indexer.test/redirect-inside HTTP/1.1"
    );
}

/// RD-1320-07, the setting off: the proxy is handed only an address checked here, so a name
/// this machine cannot resolve is not fetched — the proxy is never asked for it.
#[tokio::test]
async fn with_the_pin_a_name_only_the_proxy_could_resolve_is_not_fetched() {
    let (address, seen) = plain().await;
    let guard = rd_http::AddressPolicy::new(true);
    let network = FetchNetwork::new(Some(pinned_proxy(address)), Vec::new());

    let error = fetch_once(
        &plan("http://indexer.test/getnzb/abc", None),
        1024,
        Some(&guard),
        &network,
    )
    .await
    .expect_err("not resolvable here");

    assert_eq!(error.code(), "capture.fetch_failed");
    assert!(rd_testkit::lock(&seen).is_empty(), "the proxy was asked");
}

/// The setting off: the proxy is asked for a tunnel to the checked address, and its refusal
/// reaches the caller as `proxy.tunnel_refused` with the status.
#[tokio::test]
async fn with_the_pin_a_refused_tunnel_is_reported_with_its_status() {
    let (address, seen) = refusing().await;
    let guard = rd_http::AddressPolicy::new(true);
    let network = FetchNetwork::new(Some(pinned_proxy(address)), Vec::new());

    let error = fetch_once(
        &plan("http://93.184.216.34/file.nzb", None),
        1024,
        Some(&guard),
        &network,
    )
    .await
    .expect_err("refused");

    let message = error.into_message();
    assert_eq!(message.code, rd_http::CODE_TUNNEL_REFUSED);
    assert_eq!(
        message.params.get("status").map(String::as_str),
        Some("403")
    );
    let seen = rd_testkit::lock(&seen).clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].0, "CONNECT 93.184.216.34:80 HTTP/1.1");
}
