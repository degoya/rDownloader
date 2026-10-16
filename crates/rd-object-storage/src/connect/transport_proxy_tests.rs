//! RD-1330-01: behind the global proxy, a store whose endpoint keeps to the address rule hands
//! the proxy the address checked when the connection is made, never the name, and keeps the
//! name for the certificate; with the profile's "the proxy resolves names itself" the proxy is
//! handed the name, as before. Without a proxy profile the proxy the environment names is
//! pinned the same way, always. Loopback stands for what the rule refuses (a cloud's metadata
//! address): the recording proxy sends every tunnel to the loopback port asked for anyway.

use std::{sync::Arc, time::Duration};

use chrono::Utc;
use object_store::{client::ClientOptions, path::Path};
use rd_core::{
    ObjectAddressing, ObjectCredentialSource, ObjectStorageProfile, ObjectStorageProfileId,
    ObjectStorageProvider,
};
use rd_http::{
    AddressPolicy, EnvironmentProxy, EnvironmentRoute, NameResolution,
    test_support::{Answers, PUBLIC, TestCa, connect_proxy, entries, refusing_proxy, tls_origin},
};
use url::Url;

use super::{EnvironmentWay, NoRedirects, StoreProxy};

/// The rule of the tests: the person's network, not loopback.
fn rule() -> AddressPolicy {
    AddressPolicy::new(true)
}

fn proxy(port: u16, resolution: NameResolution, lookup: Arc<Answers>) -> Arc<StoreProxy> {
    Arc::new(StoreProxy {
        endpoint: Url::parse(&format!("http://127.0.0.1:{port}")).expect("endpoint"),
        credentials: None,
        resolution,
        lookup,
    })
}

/// A store's client behind `proxy`, trusting `custom_ca_pem`, as `object_store` asks for it.
fn client(proxy: Arc<StoreProxy>, custom_ca_pem: Vec<Vec<u8>>) -> reqwest::Client {
    NoRedirects {
        custom_ca_pem,
        timeout: Duration::from_secs(5),
        proxy: Some(proxy),
        environment: None,
        guard: Some(rule()),
    }
    .client(&ClientOptions::new().with_allow_http(true))
    .expect("client")
}

/// A store's client for `endpoint` without a proxy profile, the environment naming the
/// recording proxy on `port` — given, not set, so no other test sees the variables.
fn environment_client(port: u16, endpoint: &Url, lookup: Arc<Answers>) -> reqwest::Client {
    let proxy = format!("http://127.0.0.1:{port}");
    let way = EnvironmentWay {
        target: endpoint.clone(),
        route: EnvironmentRoute::new(
            EnvironmentProxy::given(Some(&proxy), Some(&proxy), None),
            lookup,
        ),
    };
    NoRedirects {
        custom_ca_pem: Vec::new(),
        timeout: Duration::from_secs(5),
        proxy: None,
        environment: Some(Arc::new(way)),
        guard: Some(rule()),
    }
    .client(&ClientOptions::new().with_allow_http(true))
    .expect("client")
}

async fn origin() -> rd_testkit::TestServer {
    rd_testkit::serve(axum::Router::new().fallback(|| async { "ok" })).await
}

#[tokio::test]
async fn behind_the_proxy_the_endpoint_is_reached_at_the_checked_address() {
    let origin = origin().await;
    let port = origin.addr().port();
    let (proxy_port, asked) = connect_proxy(None).await;
    let client = client(
        proxy(proxy_port, NameResolution::Here, Answers::steady()),
        Vec::new(),
    );

    let body = client
        .get(format!("http://minio.test:{port}/media"))
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body");

    assert_eq!(body, "ok");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{port} HTTP/1.1")]
    );
}

/// The acceptance case: the endpoint's name passes the check before the store is opened with
/// a public address and answers with loopback when the connection is made.
#[tokio::test]
async fn a_name_that_answers_inside_after_the_check_never_reaches_the_proxy() {
    let (proxy_port, asked) = connect_proxy(None).await;
    let lookup = Answers::rebinding();
    let endpoint = Url::parse("http://minio.test:9000/media").expect("endpoint");
    // What `endpoint::guard` checks before a store is opened.
    let checked = rd_http::check_target(&rule(), lookup.as_ref(), &endpoint)
        .await
        .expect("the check passes");
    assert_eq!(checked, [PUBLIC]);
    let client = client(proxy(proxy_port, NameResolution::Here, lookup), Vec::new());

    let error = client
        .get(endpoint.as_str())
        .send()
        .await
        .expect_err("refused");

    assert!(rd_http::is_refusal(&error), "{error:?}");
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}

/// Without a proxy profile, the proxy the environment names is handed the checked address too.
#[tokio::test]
async fn behind_the_environment_proxy_the_endpoint_is_reached_at_the_checked_address() {
    let origin = origin().await;
    let port = origin.addr().port();
    let (proxy_port, asked) = connect_proxy(None).await;
    let endpoint = Url::parse(&format!("http://minio.test:{port}/media")).expect("endpoint");
    let client = environment_client(proxy_port, &endpoint, Answers::steady());

    let body = client
        .get(endpoint.as_str())
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body");

    assert_eq!(body, "ok");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{port} HTTP/1.1")]
    );
}

/// The acceptance case behind the environment's proxy: the name passes the check with a public
/// address and answers with loopback when the connection is made; the proxy is never asked.
#[tokio::test]
async fn behind_the_environment_proxy_a_rebinding_name_reaches_nothing_inside() {
    let (proxy_port, asked) = connect_proxy(None).await;
    let lookup = Answers::rebinding();
    let endpoint = Url::parse("http://minio.test:9000/media").expect("endpoint");
    let checked = rd_http::check_target(&rule(), lookup.as_ref(), &endpoint)
        .await
        .expect("the check passes");
    assert_eq!(checked, [PUBLIC]);
    let client = environment_client(proxy_port, &endpoint, lookup);

    let error = client
        .get(endpoint.as_str())
        .send()
        .await
        .expect_err("refused");

    assert!(rd_http::is_refusal(&error), "{error:?}");
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}

/// The profile's "the proxy resolves names itself": the proxy is handed the name, as before.
#[tokio::test]
async fn with_the_proxy_resolving_names_it_is_handed_the_name() {
    let (proxy_port, asked) = connect_proxy(None).await;
    let client = client(
        proxy(proxy_port, NameResolution::AtProxy, Answers::steady()),
        Vec::new(),
    );

    let response = client
        .get("http://minio.test:9000/media")
        .send()
        .await
        .expect("response");

    // The fixture opens tunnels only; an absolute-form request is answered `502`.
    assert_eq!(response.status().as_u16(), 502);
    assert_eq!(
        entries(&asked),
        ["GET http://minio.test:9000/media HTTP/1.1"]
    );
}

/// HTTPS through the proxy: the tunnel ends at the checked address, the TLS inside it carries
/// the name as SNI, and the certificate is checked on the name with the custom CA every
/// transport trusts.
#[tokio::test]
async fn https_through_the_proxy_checks_the_certificate_on_the_name_with_the_custom_ca() {
    let ca = TestCa::new();
    let (origin, sni) = tls_origin(ca.server(&["minio.test"])).await;
    let (proxy_port, asked) = connect_proxy(None).await;
    let client = client(
        proxy(proxy_port, NameResolution::Here, Answers::steady()),
        vec![ca.pem.clone().into_bytes()],
    );

    let body = client
        .get(format!("https://minio.test:{origin}/media"))
        .send()
        .await
        .expect("response")
        .text()
        .await
        .expect("body");

    assert_eq!(body, "ok");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{origin} HTTP/1.1")]
    );
    assert_eq!(entries(&sni), ["minio.test"]);

    // A certificate for the checked address alone does not name the host.
    let (other, _) = tls_origin(ca.server(&["93.184.216.34"])).await;
    let error = client
        .get(format!("https://minio.test:{other}/media"))
        .send()
        .await
        .expect_err("the certificate does not name the host");
    assert!(!rd_http::is_refusal(&error), "{error:?}");
}

fn profile(endpoint: &str) -> ObjectStorageProfile {
    ObjectStorageProfile {
        id: ObjectStorageProfileId::new(),
        name: "behind a proxy".to_owned(),
        provider: ObjectStorageProvider::S3,
        endpoint: Some(endpoint.to_owned()),
        region: None,
        bucket: Some("media-bucket".to_owned()),
        addressing: ObjectAddressing::Path,
        credential_source: ObjectCredentialSource::Anonymous,
        access_key_id: None,
        account: None,
        ambient_custom_endpoint: false,
        secret_ref: None,
        session_token_ref: None,
        has_secret: false,
        has_session_token: false,
        checksums: false,
        enabled: true,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

/// A proxy that refuses the tunnel to the checked address: the store's failure is
/// `proxy.tunnel_refused` with the status, as for every other transport.
#[tokio::test]
async fn a_refused_tunnel_is_reported_with_the_proxys_status() {
    let (proxy_port, asked) = refusing_proxy(403).await;
    let profile = profile("http://minio.test:9000");
    let store = super::super::open(super::super::Opening {
        profile: &profile,
        bucket: "media-bucket",
        secret: None,
        session_token: None,
        proxy: Some(proxy(proxy_port, NameResolution::Here, Answers::steady())),
        custom_ca_pem: &[],
        timeout: Duration::from_secs(5),
        guard: Some(rule()),
    })
    .expect("store");

    let error = crate::head(&store, &Path::from("media.bin"))
        .await
        .expect_err("refused");

    let failure = crate::error::classify(&error, "media-bucket");
    assert_eq!(
        failure.code.as_deref(),
        Some(rd_http::CODE_TUNNEL_REFUSED),
        "{failure:?}"
    );
    assert_eq!(
        failure.params.get("status").map(String::as_str),
        Some("403")
    );
    let asked = entries(&asked);
    assert!(!asked.is_empty());
    assert!(
        asked
            .iter()
            .all(|line| line == &format!("CONNECT {PUBLIC}:9000 HTTP/1.1")),
        "{asked:?}"
    );
}
