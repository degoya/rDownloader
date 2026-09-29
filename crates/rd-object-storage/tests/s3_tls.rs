//! A custom S3 endpoint over TLS (RD-150-04).
//!
//! The endpoint's certificate comes from a CA made for the test, the way a NAS or a self-hosted
//! MinIO has one. Trusted through the custom CA every transport shares
//! (`rd_http::NetworkDefaults::custom_ca_pem`), a signed request reaches it; without that CA, or
//! with another one, the handshake is refused and no request arrives at all. There is no switch
//! that turns validation off, so none is tested.

mod support;

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::State,
    http::{Method, StatusCode, Uri},
    response::Response,
};
use rd_core::{ObjectAddressing, ObjectCredentialSource, ObjectStorageProvider};
use rd_db::NewObjectStorageProfile;
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer},
};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, server::TlsStream};

use support::{Harness, lock, respond};

const BUCKET: &str = "tls-bucket";

/// The requests that got past the handshake, as `METHOD /path`.
type Seen = Arc<Mutex<Vec<String>>>;

/// A listener that hands axum only connections whose handshake succeeded. A client that does
/// not trust the certificate aborts the handshake — the refusal under test, not a reason to
/// stop serving.
struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((stream, address)) = self.tcp.accept().await else {
                continue;
            };
            if let Ok(tls) = self.acceptor.accept(stream).await {
                return (tls, address);
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// `ListObjectsV2` of the bucket, empty: what testing a profile asks for.
async fn handle(State(seen): State<Seen>, method: Method, uri: Uri) -> Response {
    lock(&seen).push(format!("{method} {}", uri.path()));
    if uri.path().trim_matches('/') != BUCKET {
        return respond(
            StatusCode::NOT_FOUND,
            &[("content-type", "application/xml".to_owned())],
            "<Error><Code>NoSuchBucket</Code></Error>",
        );
    }
    respond(
        StatusCode::OK,
        &[("content-type", "application/xml".to_owned())],
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name>\
             <Prefix></Prefix><KeyCount>0</KeyCount><MaxKeys>1000</MaxKeys>\
             <IsTruncated>false</IsTruncated></ListBucketResult>"
        ),
    )
}

struct Endpoint {
    address: SocketAddr,
    /// The PEM of the CA that signed the endpoint's certificate.
    ca_pem: String,
    seen: Seen,
}

/// A CA of its own and a certificate for `127.0.0.1` signed by it; the S3 fixture behind it.
async fn tls_endpoint() -> Endpoint {
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    // A CA named like its leaf reads as self-signed on Windows (see rd-http's TLS fixture).
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "rDownloader S3 test CA");
    let ca_key = rcgen::KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca cert");
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);

    let mut server_params =
        rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).expect("server params");
    server_params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(std::net::IpAddr::from([
            127, 0, 0, 1,
        ])));
    let server_key = rcgen::KeyPair::generate().expect("server key");
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .expect("server cert");

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(server_cert.der().to_vec())],
            PrivateKeyDer::try_from(server_key.serialize_der()).expect("server key der"),
        )
        .expect("server config");
    // The fixture speaks HTTP/1.1 only (axum without `http2`).
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = tcp.local_addr().expect("address");
    let seen = Seen::default();
    let app = Router::new().fallback(handle).with_state(seen.clone());
    let listener = TlsListener {
        tcp,
        acceptor: TlsAcceptor::from(Arc::new(config)),
    };
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Endpoint {
        address,
        ca_pem: ca_cert.pem(),
        seen,
    }
}

/// A service whose one profile names `https://<address>`, trusting `custom_ca_pem`.
async fn harness(address: SocketAddr, custom_ca_pem: Vec<Vec<u8>>) -> Harness {
    Harness::start_with_network(
        NewObjectStorageProfile {
            name: "tls".to_owned(),
            provider: ObjectStorageProvider::S3,
            endpoint: Some(format!("https://{address}")),
            region: Some("us-east-1".to_owned()),
            bucket: Some(BUCKET.to_owned()),
            addressing: ObjectAddressing::Path,
            credential_source: ObjectCredentialSource::Static,
            access_key_id: Some("AKIDEXAMPLE".to_owned()),
            account: None,
            secret_ref: None,
            session_token_ref: None,
            checksums: false,
            enabled: true,
        },
        Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"),
        "s3",
        BUCKET,
        rd_http::NetworkDefaults {
            custom_ca_pem,
            ..rd_http::NetworkDefaults::default()
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_custom_endpoint_is_reached_over_tls_through_the_custom_ca() {
    let endpoint = tls_endpoint().await;
    let harness = harness(endpoint.address, vec![endpoint.ca_pem.into_bytes()]).await;
    let failure = harness
        .service
        .test_profile(&harness.profile)
        .await
        .expect("test");
    assert!(failure.is_none(), "{failure:?}");
    let seen = lock(&endpoint.seen).clone();
    assert!(
        seen.iter()
            .any(|request| request.starts_with(&format!("GET /{BUCKET}"))),
        "{seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_endpoint_the_trusted_roots_do_not_vouch_for_is_refused() {
    let endpoint = tls_endpoint().await;
    // The system roots alone, and the system roots plus a CA that signed something else: the
    // endpoint's certificate validates under neither.
    let other = tls_endpoint().await;
    for custom_ca_pem in [Vec::new(), vec![other.ca_pem.clone().into_bytes()]] {
        let harness = harness(endpoint.address, custom_ca_pem).await;
        let failure = harness
            .service
            .test_profile(&harness.profile)
            .await
            .expect("test")
            .expect("an untrusted certificate must fail the test");
        assert_eq!(
            failure.code.as_deref(),
            Some(rd_object_storage::error::CONNECT_FAILED),
            "{failure:?}"
        );
    }
    assert!(
        lock(&endpoint.seen).is_empty(),
        "no request may pass a refused handshake"
    );
}
