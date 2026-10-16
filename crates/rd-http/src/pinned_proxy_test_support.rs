//! What a test of a request behind a proxy talks to, here and in the crates that pin through
//! this one (RD-1330-01): a lookup that answers as a rebinding name does, an HTTP proxy that
//! records which target it was asked for, a CA of the test's own and a TLS origin that records
//! the SNI. The proxy sends every tunnel to the loopback port asked for, whatever the address,
//! so a request to a "public" address still reaches a local origin. Another crate's tests take
//! it with the feature `test-support`; the service never does.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_rustls::TlsAcceptor;

use crate::{HostLookup, LookupFuture};

/// The address a target name answers the check with.
pub const PUBLIC: IpAddr = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));

/// What a fixture recorded, one entry per request or connection.
pub type Log = Arc<Mutex<Vec<String>>>;

/// What `log` holds now.
#[must_use]
pub fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn record(log: &Log, entry: String) {
    log.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(entry);
}

/// Answers every target name with each of `answers` in turn, the last one from then on;
/// `proxy.test` (a SOCKS5 proxy's own name) with loopback, and an address with itself.
pub struct Answers {
    answers: Vec<IpAddr>,
    calls: AtomicUsize,
}

impl Answers {
    /// A name that always answers with [`PUBLIC`].
    #[must_use]
    pub fn steady() -> Arc<Self> {
        Self::with(vec![PUBLIC])
    }

    /// A rebinding name: [`PUBLIC`] for the check, loopback from the next lookup on.
    #[must_use]
    pub fn rebinding() -> Arc<Self> {
        Self::with(vec![PUBLIC, IpAddr::V4(Ipv4Addr::LOCALHOST)])
    }

    /// A name with no address here, as in a network where only the proxy resolves.
    #[must_use]
    pub fn nothing() -> Arc<Self> {
        Self::with(Vec::new())
    }

    fn with(answers: Vec<IpAddr>) -> Arc<Self> {
        Arc::new(Self {
            answers,
            calls: AtomicUsize::new(0),
        })
    }
}

impl HostLookup for Answers {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a> {
        let answer = if let Ok(address) = host.parse::<IpAddr>() {
            vec![address]
        } else if host == "proxy.test" {
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        } else {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.answers
                .get(call)
                .or(self.answers.last())
                .copied()
                .into_iter()
                .collect()
        };
        Box::pin(async move { Ok(answer) })
    }
}

/// A certificate authority of the test's own, which no platform store trusts.
pub struct TestCa {
    pub pem: String,
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
}

impl TestCa {
    /// # Panics
    /// When the certificate cannot be made, which only a test sees.
    #[must_use]
    pub fn new() -> Self {
        let mut params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        // A name of its own: Windows reads a leaf whose subject equals its issuer as
        // self-signed (see `tests/auth_profile_client.rs`).
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            "rDownloader pinned proxy test CA",
        );
        let key = rcgen::KeyPair::generate().expect("ca key");
        let pem = params.self_signed(&key).expect("ca cert").pem();
        Self {
            pem,
            issuer: rcgen::Issuer::new(params, key),
        }
    }

    /// A server configuration with a certificate for `names`, each a DNS name or an address.
    ///
    /// # Panics
    /// When the certificate cannot be made, which only a test sees.
    #[must_use]
    pub fn server(&self, names: &[&str]) -> Arc<ServerConfig> {
        let names: Vec<String> = names.iter().map(|name| (*name).to_owned()).collect();
        let params = rcgen::CertificateParams::new(names).expect("server params");
        let key = rcgen::KeyPair::generate().expect("server key");
        let certificate = params.signed_by(&key, &self.issuer).expect("server cert");
        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(certificate.der().to_vec())],
                PrivateKeyDer::try_from(key.serialize_der()).expect("server key der"),
            )
            .expect("server config");
        // The hand-written response below is HTTP/1.1; reqwest would offer h2 as well.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(config)
    }
}

impl Default for TestCa {
    fn default() -> Self {
        Self::new()
    }
}

const OK: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";

/// A TLS origin that answers `ok` and records the SNI each handshake carried (`-` for none).
pub async fn tls_origin(config: Arc<ServerConfig>) -> (u16, Log) {
    let (listener, port) = listen().await;
    let log = Log::default();
    let seen = Arc::clone(&log);
    let acceptor = TlsAcceptor::from(config);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (acceptor, seen) = (acceptor.clone(), Arc::clone(&seen));
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let sni = tls.get_ref().1.server_name().unwrap_or("-").to_owned();
                record(&seen, sni);
                read_head(&mut tls).await;
                let _ = tls.write_all(OK).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (port, log)
}

/// An HTTP proxy that records each request line, with ` | <Proxy-Authorization>` when one
/// came, and opens every `CONNECT` tunnel to the loopback port asked for. Anything else gets
/// `502`. Over TLS with `tls`.
pub async fn connect_proxy(tls: Option<Arc<ServerConfig>>) -> (u16, Log) {
    let (listener, port) = listen().await;
    let log = Log::default();
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            match &tls {
                Some(config) => {
                    let acceptor = TlsAcceptor::from(Arc::clone(config));
                    tokio::spawn(async move {
                        if let Ok(stream) = acceptor.accept(stream).await {
                            tunnel(stream, seen).await;
                        }
                    });
                }
                None => {
                    tokio::spawn(tunnel(stream, seen));
                }
            }
        }
    });
    (port, log)
}

async fn tunnel<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S, seen: Log) {
    let head = read_head(&mut stream).await;
    let line = head.lines().next().unwrap_or_default().to_owned();
    let authorization = head.lines().find_map(|header| {
        let (name, value) = header.split_once(':')?;
        name.eq_ignore_ascii_case("proxy-authorization")
            .then(|| value.trim().to_owned())
    });
    let entry = match authorization {
        Some(value) => format!("{line} | {value}"),
        None => line.clone(),
    };
    record(&seen, entry);
    let port = line
        .strip_prefix("CONNECT ")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|target| target.rsplit(':').next())
        .and_then(|port| port.parse::<u16>().ok());
    let origin = match port {
        Some(port) => TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.ok(),
        None => None,
    };
    let Some(mut origin) = origin else {
        let _ = stream
            .write_all(
                b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await;
        return;
    };
    let _ = stream
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut origin).await;
}

/// An HTTP proxy that answers every request with `status` and records each request line.
pub async fn refusing_proxy(status: u16) -> (u16, Log) {
    let (listener, port) = listen().await;
    let log = Log::default();
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let head = read_head(&mut stream).await;
            let line = head.lines().next().unwrap_or_default().to_owned();
            record(&seen, line);
            let answer = format!("HTTP/1.1 {status} Refused\r\ncontent-length: 0\r\n\r\n");
            let _ = stream.write_all(answer.as_bytes()).await;
        }
    });
    (port, log)
}

/// A listener on a free loopback port, and the port.
///
/// # Panics
/// When no port can be bound, which only a test sees.
pub async fn listen() -> (TcpListener, u16) {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    (listener, port)
}

/// Reads a request head byte by byte, so nothing after it is taken from the stream.
pub async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte).await {
            Ok(1) => head.push(byte[0]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}
