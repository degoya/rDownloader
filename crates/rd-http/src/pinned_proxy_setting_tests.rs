//! RD-1320-07: the profile's "the proxy resolves names itself", and a proxy that refuses the
//! tunnel to a checked address.

use std::time::Duration;

use url::Url;

use super::fixtures::{
    Answers, PUBLIC, client, connect_proxy, entries, fetch, plain_origin, refusing_proxy,
    socks_proxy,
};
use super::*;

/// A client under the public rule behind `endpoint` as [`with_proxy`] builds it, with the
/// system's lookup — which a name the proxy resolves never reaches.
fn resolving_at_proxy(endpoint: &str) -> reqwest::Client {
    let builder =
        crate::client_builder(crate::ClientPurpose::Direct).connect_timeout(Duration::from_secs(5));
    with_proxy(
        builder,
        &Url::parse(endpoint).expect("endpoint"),
        None,
        Some(&AddressPolicy::new(false)),
        NameResolution::AtProxy,
        &[],
    )
    .expect("builder")
    .build()
    .expect("client")
}

/// The setting on is the behaviour before the pin: the proxy is handed the name and resolves
/// it itself, so a name this machine cannot resolve still goes out.
#[tokio::test]
async fn with_the_proxy_resolving_names_it_is_handed_the_name() {
    let (proxy, asked) = connect_proxy(None).await;
    let client = resolving_at_proxy(&format!("http://127.0.0.1:{proxy}"));

    let response = client
        .get("http://files.test:8080/file")
        .send()
        .await
        .expect("response");

    // The fixture opens tunnels only; an absolute-form request is answered `502`.
    assert_eq!(response.status().as_u16(), 502);
    assert_eq!(
        entries(&asked),
        ["GET http://files.test:8080/file HTTP/1.1"]
    );
}

#[tokio::test]
async fn with_the_proxy_resolving_names_a_socks5h_proxy_is_handed_the_name() {
    let (origin, _) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = socks_proxy().await;
    let client = resolving_at_proxy(&format!("socks5h://127.0.0.1:{proxy}"));

    let body = fetch(&client, &format!("http://files.test:{port}/file")).await;

    assert_eq!(body, "ok");
    assert_eq!(entries(&asked), [format!("files.test:{port}")]);
}

/// The setting off, the default: `socks5h` would hand the proxy the name, and under the rule
/// is connected as `socks5` instead.
#[tokio::test]
async fn by_default_a_socks5h_proxy_is_handed_the_checked_address() {
    let (origin, _) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = socks_proxy().await;
    let client = client(
        &format!("socks5h://proxy.test:{proxy}"),
        None,
        Answers::steady(),
        None,
        &[],
    );

    let body = fetch(&client, &format!("http://files.test:{port}/file")).await;

    assert_eq!(body, "ok");
    assert_eq!(entries(&asked), [format!("{PUBLIC}:{port}")]);
}

/// Squid permits `CONNECT` only to port 443 by default; its refusal reaches the caller as
/// `proxy.tunnel_refused` with the status, not as a bare SOCKS failure.
#[tokio::test]
async fn a_refused_tunnel_is_reported_with_the_proxys_status() {
    let (proxy, asked) = refusing_proxy(403).await;
    let client = client(
        &format!("http://127.0.0.1:{proxy}"),
        None,
        Answers::steady(),
        None,
        &[],
    );

    let error = client
        .get("http://files.test:8080/file")
        .send()
        .await
        .expect_err("refused");

    assert_eq!(crate::tunnel_refusal(&error), Some(403), "{error:?}");
    assert!(!crate::is_refusal(&error));
    assert_eq!(entries(&asked), [format!("CONNECT {PUBLIC}:8080 HTTP/1.1")]);
    let crate::HttpDownloadError::Failure(failure) = crate::engine::network_failure(error) else {
        panic!("a refused tunnel is a coded failure");
    };
    assert_eq!(failure.code.as_deref(), Some(crate::CODE_TUNNEL_REFUSED));
    assert_eq!(
        failure.params.get("status").map(String::as_str),
        Some("403")
    );
    assert!(matches!(failure.category, rd_core::FailureKind::Permanent));
}

/// A download whose name this machine cannot resolve, behind the pinned proxy: it fails as
/// `proxy.name_unresolved` with the host, retried, and the proxy is never asked.
#[tokio::test]
async fn a_name_only_the_proxy_could_resolve_fails_the_download_with_its_code() {
    let (proxy, asked) = connect_proxy(None).await;
    let client = client(
        &format!("http://127.0.0.1:{proxy}"),
        None,
        Answers::nothing(),
        None,
        &[],
    );

    let error = client
        .get("http://files.test:8080/file")
        .send()
        .await
        .expect_err("unresolved");

    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
    let crate::HttpDownloadError::Failure(failure) = crate::engine::network_failure(error) else {
        panic!("an unresolved name is a coded failure");
    };
    assert_eq!(failure.code.as_deref(), Some(crate::CODE_NAME_UNRESOLVED));
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("files.test")
    );
    assert!(failure.category.is_retryable(), "{failure:?}");
}

/// A login the proxy wants and was not given is a `407`, told apart from the rest.
#[tokio::test]
async fn a_proxy_that_wants_a_login_is_reported_as_407() {
    let (proxy, _) = refusing_proxy(407).await;
    let client = client(
        &format!("http://127.0.0.1:{proxy}"),
        None,
        Answers::steady(),
        None,
        &[],
    );

    let error = client
        .get("https://files.test/file")
        .send()
        .await
        .expect_err("refused");

    assert_eq!(crate::tunnel_refusal(&error), Some(407), "{error:?}");
}

/// The shared rule of what is judged before the request: a literal always, a name only when
/// the proxy resolves it — then one this machine cannot resolve passes, for the proxy.
#[tokio::test]
async fn before_the_request_a_name_is_judged_only_when_the_proxy_resolves_it() {
    let policy = AddressPolicy::new(false);
    let inside = Answers::rebinding();
    // The rebinding lookup's first answer is public; take it, so the name answers inside.
    let target = Url::parse("http://files.test/").expect("target");
    crate::check_target(&policy, inside.as_ref(), &target)
        .await
        .expect("the first answer");
    let check = |target: &'static str, resolution| {
        let (policy, inside) = (policy.clone(), Arc::clone(&inside));
        async move {
            let target = Url::parse(target).expect("target");
            check_before_request(&policy, inside.as_ref(), &target, resolution).await
        }
    };

    assert!(
        check("http://127.0.0.1/", NameResolution::Here)
            .await
            .is_err()
    );
    assert!(
        check("http://127.0.0.1/", NameResolution::AtProxy)
            .await
            .is_err()
    );
    assert!(
        check("http://files.test/", NameResolution::Here)
            .await
            .is_ok()
    );
    assert!(
        check("http://files.test/", NameResolution::AtProxy)
            .await
            .is_err()
    );
    let nowhere = check_before_request(
        &policy,
        &crate::SystemLookup,
        &Url::parse("http://unresolvable.invalid/").expect("target"),
        NameResolution::AtProxy,
    )
    .await;
    assert!(nowhere.is_ok());
}

#[test]
fn a_profile_says_where_names_are_resolved() {
    assert_eq!(NameResolution::of(None), NameResolution::Here);
    assert_eq!(
        NameResolution::resolving_at_proxy(true),
        NameResolution::AtProxy
    );
    assert_eq!(
        NameResolution::resolving_at_proxy(false),
        NameResolution::Here
    );
}
