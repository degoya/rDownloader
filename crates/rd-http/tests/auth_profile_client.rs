//! Local-fixture coverage for the three credential methods.
//!
//! Basic and Bearer are checked against a plain HTTP fixture, including that the header
//! does not survive a redirect to a foreign host. mTLS is checked against a real TLS
//! listener with a client-certificate verifier: if reqwest fails to present the identity
//! the handshake fails, which is a more direct assertion than inspecting any header.

use std::sync::Arc;

use axum::{Router, response::Redirect, routing::get};
use reqwest::cookie::Jar;
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer},
    server::WebPkiClientVerifier,
};
use secrecy::SecretString;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_rustls::TlsAcceptor;

use rd_http::{AuthMaterial, ClientContext, ClientKey, ClientPool};

fn key(profile: Option<rd_core::AuthProfileId>) -> ClientKey {
    ClientKey {
        proxy_profile_id: None,
        account_id: None,
        cookie_ref: None,
        auth_profile_id: profile,
        auth_revision: 1,
        replay_scope: None,
        tls_revision: 0,
    }
}

fn context(auth: Option<AuthMaterial>, custom_ca_pem: Vec<Vec<u8>>) -> ClientContext {
    ClientContext {
        key: key(auth.as_ref().map(|_| rd_core::AuthProfileId::new())),
        proxy: None,
        proxy_credentials: None,
        cookie_jar: Arc::new(Jar::default()),
        custom_ca_pem,
        auth,
        replay_scope: None,
    }
}

/// Serves `/guarded` (echoes the Authorization header it saw) and `/away` (redirects to
/// `elsewhere`), on an ephemeral port.
async fn http_fixture(elsewhere: Option<String>) -> String {
    let app = Router::new()
        .route(
            "/guarded",
            get(|headers: axum::http::HeaderMap| async move {
                headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("<none>")
                    .to_owned()
            }),
        )
        .route(
            "/away",
            get(move || {
                let target = elsewhere.clone().unwrap_or_default();
                async move { Redirect::temporary(&target) }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}")
}

#[tokio::test]
async fn a_bearer_token_reaches_the_target_but_not_a_foreign_redirect() {
    // The header is applied per request, exactly as the scheduler does it.
    let headers = [(
        "authorization".to_owned(),
        "Bearer profile-token".to_owned(),
    )];
    let foreign = http_fixture(None).await;
    let origin = http_fixture(Some(format!("{foreign}/guarded"))).await;
    let client = ClientPool::default()
        .get_or_create(context(None, Vec::new()))
        .await
        .expect("client");

    let mut request = client.get(format!("{origin}/guarded"));
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let seen = request
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body");
    assert_eq!(seen, "Bearer profile-token");

    // reqwest strips Authorization on any host/port/scheme change, which is what keeps a
    // credential from following a hoster's handoff to a foreign CDN.
    let mut request = client.get(format!("{origin}/away"));
    for (name, value) in &headers {
        request = request.header(name, value);
    }
    let after_redirect = request
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body");
    assert_eq!(
        after_redirect, "<none>",
        "the credential must not survive a redirect to another host"
    );
}

#[tokio::test]
async fn basic_credentials_are_encoded_the_way_the_scheduler_sends_them() {
    let origin = http_fixture(None).await;
    let client = ClientPool::default()
        .get_or_create(context(None, Vec::new()))
        .await
        .expect("client");
    let seen = client
        .get(format!("{origin}/guarded"))
        .header("authorization", "Basic dXNlcjpwYXNzd29yZA==")
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body");
    assert_eq!(seen, "Basic dXNlcjpwYXNzd29yZA==");
}

struct TlsFixture {
    address: std::net::SocketAddr,
    ca_pem: String,
    client_identity_pem: String,
}

/// Starts a TLS listener that *requires* a client certificate signed by the test CA.
async fn mtls_fixture() -> TlsFixture {
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    // rcgen names every certificate `CN=rcgen self signed cert`. Windows reads a leaf whose
    // subject equals its issuer as self-signed and fails it on its own signature
    // (TRUST_E_CERT_SIGNATURE), so the CA gets a name of its own.
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "rDownloader test CA");
    let ca_key = rcgen::KeyPair::generate().expect("ca key");
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca cert");
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);

    // rustls accepts an IP server name, so the test needs no hosts-file entry.
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

    let client_params = rcgen::CertificateParams::new(vec!["rdownloader-client".to_owned()])
        .expect("client params");
    let client_key = rcgen::KeyPair::generate().expect("client key");
    let client_cert = client_params
        .signed_by(&client_key, &issuer)
        .expect("client cert");

    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca_cert.der().to_vec()))
        .expect("trust ca");
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .expect("client verifier");
    let mut config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from(server_cert.der().to_vec())],
            PrivateKeyDer::try_from(server_key.serialize_der()).expect("server key der"),
        )
        .expect("server config");
    // reqwest advertises h2 by default; pinning http/1.1 keeps the hand-written response
    // below framed the way the client expects.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buffer = [0_u8; 2048];
                let _ = tls.read(&mut buffer).await;
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });

    TlsFixture {
        address,
        ca_pem: ca_cert.pem(),
        client_identity_pem: format!("{}{}", client_key.serialize_pem(), client_cert.pem()),
    }
}

#[tokio::test]
async fn a_client_certificate_is_presented_and_is_required() {
    let fixture = mtls_fixture().await;
    let url = format!("https://{}/file", fixture.address);
    let ca = vec![fixture.ca_pem.clone().into_bytes()];

    // The custom CA travels the production path: build_client already feeds it to
    // Certificate::from_pem, so this exercises the real code rather than a test shortcut.
    let with_identity = ClientPool::default()
        .get_or_create(context(
            Some(AuthMaterial {
                identity_pem: SecretString::from(fixture.client_identity_pem.clone()),
            }),
            ca.clone(),
        ))
        .await
        .expect("client");
    let body = with_identity
        .get(&url)
        .send()
        .await
        .expect("mTLS request")
        .text()
        .await
        .expect("body");
    assert_eq!(body, "ok");

    // Without the identity the verifier rejects the handshake, which proves the server is
    // really enforcing client authentication rather than accepting anyone.
    let without_identity = ClientPool::default()
        .get_or_create(context(None, ca))
        .await
        .expect("client");
    assert!(
        without_identity.get(&url).send().await.is_err(),
        "the fixture must require a client certificate"
    );
}

#[tokio::test]
async fn a_certificate_bearing_client_refuses_to_leave_its_origin() {
    // A client certificate is a property of the client, so nothing in reqwest stops it
    // from being offered to a redirect target during the handshake. This is the only case
    // that needs the origin-bounded redirect policy, and this is the test for it. The
    // second fixture listens on another port, which is already a different origin.
    let foreign = http_fixture(None).await;
    let origin = http_fixture(Some(format!("{foreign}/guarded"))).await;
    let identity = mtls_fixture().await.client_identity_pem;

    let client = ClientPool::default()
        .get_or_create(context(
            Some(AuthMaterial {
                identity_pem: SecretString::from(identity),
            }),
            Vec::new(),
        ))
        .await
        .expect("client");

    // Same origin: an ordinary request still works.
    let in_scope = client
        .get(format!("{origin}/guarded"))
        .send()
        .await
        .expect("in-scope request");
    assert!(in_scope.status().is_success());

    // The fixture redirects to another port, which is outside the scope host:port.
    let error = client
        .get(format!("{origin}/away"))
        .send()
        .await
        .expect_err("a redirect out of the origin must fail");
    assert!(
        error.is_redirect() || error.to_string().contains("origin"),
        "unexpected error: {error}"
    );
}
