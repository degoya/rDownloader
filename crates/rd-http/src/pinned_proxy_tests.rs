//! RD-1320-07: behind a proxy, a guarded client hands the proxy the address it checked, never
//! the name, and keeps the name for `Host`, SNI and the certificate.

use std::sync::Arc;

use secrecy::SecretString;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use url::Url;

use super::fixtures::{
    Answers, PUBLIC, TestCa, client, connect_proxy, entries, fetch, plain_origin, socks_proxy,
    socks_proxy_with, tls_origin,
};
use super::*;

#[tokio::test]
async fn plain_http_goes_to_the_checked_address_with_the_name_as_host() {
    let (origin, seen) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = connect_proxy(None).await;
    let credentials = ProxyCredentials {
        username: "user".to_owned(),
        password: SecretString::from("pass"),
    };
    let client = client(
        &format!("http://127.0.0.1:{proxy}"),
        Some(&credentials),
        Answers::steady(),
        None,
        &[],
    );

    let body = fetch(&client, &format!("http://files.test:{port}/file")).await;

    assert_eq!(body, "ok");
    // The credentials travel on the tunnel request alone, never to the server.
    assert_eq!(
        entries(&asked),
        [format!(
            "CONNECT {PUBLIC}:{port} HTTP/1.1 | Basic dXNlcjpwYXNz"
        )]
    );
    assert_eq!(
        entries(&seen),
        [format!(
            "GET /file host=files.test:{port} proxy-authorization=-"
        )]
    );
}

/// The acceptance case: the name passes the check with a public address and answers with
/// loopback when the connection is made. The proxy is never asked for anything.
#[tokio::test]
async fn a_name_that_answers_inside_after_the_check_never_reaches_the_proxy() {
    let (origin, seen) = plain_origin().await;
    let (proxy, asked) = connect_proxy(None).await;
    let answers = Answers::rebinding();
    let target =
        Url::parse(&format!("http://files.test:{}/file", origin.addr().port())).expect("target");
    let checked = crate::check_target(&AddressPolicy::new(false), answers.as_ref(), &target)
        .await
        .expect("the check passes");
    assert_eq!(checked, [PUBLIC]);
    let client = client(
        &format!("http://127.0.0.1:{proxy}"),
        None,
        answers,
        None,
        &[],
    );

    let error = client
        .get(target.as_str())
        .send()
        .await
        .expect_err("refused");

    assert!(crate::is_refusal(&error), "{error:?}");
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
    assert!(entries(&seen).is_empty());
}

#[tokio::test]
async fn https_through_the_proxy_sends_the_name_as_sni_and_checks_the_certificate_on_it() {
    let ca = TestCa::new();
    let (origin, sni) = tls_origin(ca.server(&["files.test"])).await;
    let (proxy, asked) = connect_proxy(None).await;
    let client = client(
        &format!("http://127.0.0.1:{proxy}"),
        None,
        Answers::steady(),
        Some(&ca.pem),
        &[],
    );

    let body = fetch(&client, &format!("https://files.test:{origin}/file")).await;

    assert_eq!(body, "ok");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{origin} HTTP/1.1")]
    );
    assert_eq!(entries(&sni), ["files.test"]);
}

/// The tunnel ends at the checked address, but the certificate must still name the host.
#[tokio::test]
async fn a_certificate_for_the_checked_address_alone_is_refused() {
    let ca = TestCa::new();
    let (origin, _) = tls_origin(ca.server(&["93.184.216.34"])).await;
    let (proxy, asked) = connect_proxy(None).await;
    let client = client(
        &format!("http://127.0.0.1:{proxy}"),
        None,
        Answers::steady(),
        Some(&ca.pem),
        &[],
    );

    let error = client
        .get(format!("https://files.test:{origin}/file"))
        .send()
        .await
        .expect_err("the certificate does not name the host");

    assert!(!crate::is_refusal(&error), "{error:?}");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{origin} HTTP/1.1")]
    );
}

/// An `https://` proxy is reached over TLS, its certificate checked with the custom CA.
#[tokio::test]
async fn an_https_proxy_is_reached_over_tls_with_the_custom_ca() {
    let ca = TestCa::new();
    let (origin, seen) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = connect_proxy(Some(ca.server(&["127.0.0.1"]))).await;
    let client = client(
        &format!("https://127.0.0.1:{proxy}"),
        None,
        Answers::steady(),
        None,
        &[ca.pem.clone().into_bytes()],
    );

    let body = fetch(&client, &format!("http://files.test:{port}/file")).await;

    assert_eq!(body, "ok");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{port} HTTP/1.1")]
    );
    assert_eq!(entries(&seen).len(), 1);
}

/// A `socks5://` proxy is handed the checked address by the bridge; its own name, on loopback
/// here, is resolved by the bridge, outside the rule that judges targets.
#[tokio::test]
async fn a_socks5_proxy_is_handed_the_checked_address() {
    let (origin, _) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = socks_proxy().await;
    let client = client(
        &format!("socks5://proxy.test:{proxy}"),
        None,
        Answers::steady(),
        None,
        &[],
    );

    let body = fetch(&client, &format!("http://files.test:{port}/file")).await;

    assert_eq!(body, "ok");
    assert_eq!(entries(&asked), [format!("{PUBLIC}:{port}")]);
}

#[tokio::test]
async fn a_socks5_proxy_is_never_asked_for_a_name_that_answers_inside_after_the_check() {
    let (proxy, asked) = socks_proxy().await;
    let answers = Answers::rebinding();
    let target = Url::parse("http://files.test:8080/file").expect("target");
    crate::check_target(&AddressPolicy::new(false), answers.as_ref(), &target)
        .await
        .expect("the check passes");
    let client = client(
        &format!("socks5://proxy.test:{proxy}"),
        None,
        answers,
        None,
        &[],
    );

    let error = client
        .get(target.as_str())
        .send()
        .await
        .expect_err("refused");

    assert!(crate::is_refusal(&error), "{error:?}");
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}

/// The proxy's own name is the person's choice, never a pass for a target: a stranger's link
/// to `proxy.test` — the SOCKS5 proxy's name, answering with loopback — is judged like any
/// other, and the proxy is never asked for it (RD-1320-07 follow-up).
#[tokio::test]
async fn a_target_named_like_the_socks5_proxy_is_judged_like_any_other() {
    let (proxy, asked) = socks_proxy().await;
    let client = client(
        &format!("socks5://proxy.test:{proxy}"),
        None,
        Answers::steady(),
        None,
        &[],
    );

    let error = client
        .get("http://proxy.test:8080/")
        .send()
        .await
        .expect_err("refused");

    assert!(crate::is_refusal(&error), "{error:?}");
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}

#[tokio::test]
async fn a_socks5_proxy_with_a_login_is_signed_in() {
    let (origin, _) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = socks_proxy_with(Some(("user", "pass"))).await;
    let credentials = ProxyCredentials {
        username: "user".to_owned(),
        password: SecretString::from("pass"),
    };
    let client = client(
        &format!("socks5://proxy.test:{proxy}"),
        Some(&credentials),
        Answers::steady(),
        None,
        &[],
    );

    let body = fetch(&client, &format!("http://files.test:{port}/file")).await;

    assert_eq!(body, "ok");
    assert_eq!(entries(&asked), [format!("user@{PUBLIC}:{port}")]);
}

/// A SOCKS5 refusal of the login is reported as an HTTP proxy's would be: `407`.
#[tokio::test]
async fn a_socks5_proxy_refusing_the_login_is_reported_as_407() {
    let (proxy, asked) = socks_proxy_with(Some(("user", "pass"))).await;
    let credentials = ProxyCredentials {
        username: "user".to_owned(),
        password: SecretString::from("wrong"),
    };
    let client = client(
        &format!("socks5://proxy.test:{proxy}"),
        Some(&credentials),
        Answers::steady(),
        None,
        &[],
    );

    let error = client
        .get("http://files.test:8080/file")
        .send()
        .await
        .expect_err("refused");

    assert_eq!(crate::tunnel_refusal(&error), Some(407), "{error:?}");
    assert!(entries(&asked).is_empty());
}

/// The pool builds the pin into a guarded client behind a proxy profile, over the system's
/// lookup: a literal address goes through, a loopback one is refused before the proxy.
#[tokio::test]
async fn the_pool_pins_a_guarded_client_behind_a_proxy() {
    let (origin, _) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = connect_proxy(None).await;
    let context = crate::ClientContext {
        key: crate::ClientKey {
            proxy_profile_id: None,
            account_id: None,
            cookie_ref: None,
            auth_profile_id: None,
            auth_revision: 0,
            replay_scope: None,
            tls_revision: 0,
            address_policy: Some(AddressPolicy::new(false)),
        },
        proxy: Some(rd_core::ProxyProfile {
            id: rd_core::ProxyProfileId::new(),
            name: "test".to_owned(),
            kind: rd_core::ProxyKind::Http,
            endpoint: Url::parse(&format!("http://127.0.0.1:{proxy}")).expect("endpoint"),
            username: None,
            secret_ref: None,
            has_credentials: false,
            resolves_names: false,
        }),
        proxy_credentials: None,
        cookie_jar: Arc::new(reqwest::cookie::Jar::default()),
        custom_ca_pem: Vec::new(),
        auth: None,
        replay_scope: None,
    };
    let client = crate::ClientPool::default()
        .get_or_create(context)
        .await
        .expect("client");

    let body = fetch(&client, &format!("http://{PUBLIC}:{port}/file")).await;
    assert_eq!(body, "ok");
    let error = client
        .get(format!("http://127.0.0.1:{port}/file"))
        .send()
        .await
        .expect_err("refused");

    assert!(crate::is_refusal(&error), "{error:?}");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{port} HTTP/1.1")]
    );
}

/// Greets the bridge at `address` with `password`; the stream is past authentication when
/// the bridge accepted it.
async fn greet(address: &str, password: &[u8]) -> (TcpStream, [u8; 2]) {
    let mut stream = TcpStream::connect(address).await.expect("connect");
    stream.write_all(&[5, 1, 2]).await.expect("greeting");
    let mut chosen = [0_u8; 2];
    stream.read_exact(&mut chosen).await.expect("method");
    assert_eq!(chosen, [5, 2]);
    let length = u8::try_from(password.len()).expect("length");
    let mut request = vec![1, 2, b'r', b'd', length];
    request.extend_from_slice(password);
    stream.write_all(&request).await.expect("authentication");
    let mut answer = [0_u8; 2];
    stream.read_exact(&mut answer).await.expect("answer");
    (stream, answer)
}

async fn reply_code(stream: &mut TcpStream, request: &[u8]) -> u8 {
    stream.write_all(request).await.expect("request");
    let mut reply = [0_u8; 10];
    stream.read_exact(&mut reply).await.expect("reply");
    reply[1]
}

/// Anything else on this machine can reach the bridge's port: it serves its own client
/// alone, and only a checked address.
#[tokio::test]
async fn the_bridge_serves_its_own_client_and_only_a_permitted_address() {
    let (proxy, asked) = connect_proxy(None).await;
    let endpoint = Url::parse(&format!("http://127.0.0.1:{proxy}")).expect("endpoint");
    let upstream =
        Upstream::new(&endpoint, None, &[], Arc::new(crate::SystemLookup)).expect("upstream");
    let tickets = Arc::new(crate::tunnel_refusal::Tickets::default());
    let bridge = Bridge::start(upstream, AddressPolicy::new(false), tickets).expect("bridge");
    let address = format!("127.0.0.1:{}", bridge.url().port().expect("port"));
    let token = bridge.url().password().expect("token").as_bytes().to_vec();

    let mut anonymous = TcpStream::connect(&address).await.expect("connect");
    anonymous.write_all(&[5, 1, 0]).await.expect("greeting");
    let mut chosen = [0_u8; 2];
    anonymous.read_exact(&mut chosen).await.expect("method");
    assert_eq!(chosen, [5, 0xFF]);

    let (_, answer) = greet(&address, b"not the token").await;
    assert_eq!(answer, [1, 1]);

    let (mut by_name, answer) = greet(&address, &token).await;
    assert_eq!(answer, [1, 0]);
    // The bridge answers at the address type and reads no further, so the test sends no
    // more: unread bytes would turn its close into a reset that can swallow the reply.
    assert_eq!(reply_code(&mut by_name, &[5, 1, 0, 3]).await, 8);

    let (mut inside, _) = greet(&address, &token).await;
    let request = [5, 1, 0, 1, 127, 0, 0, 1, 0, 80];
    assert_eq!(reply_code(&mut inside, &request).await, 2);

    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}
