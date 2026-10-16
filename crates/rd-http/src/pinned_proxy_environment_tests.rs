//! RD-1330-01: the proxy the environment names is read as reqwest reads it and pinned like a
//! profile's, and a name only the proxy could resolve fails with its own code.

use url::Url;

use super::fixtures::{Answers, PUBLIC, connect_proxy, entries, fetch, plain_origin};
use super::*;
use crate::{AddressPolicy, ClientPurpose};

fn url(text: &str) -> Url {
    Url::parse(text).expect("url")
}

/// A service client for `target` the way [`EnvironmentRoute::guarded`] builds it.
fn client(route: &EnvironmentRoute, target: &Url) -> reqwest::Client {
    route
        .guarded(
            crate::client_builder(ClientPurpose::Service),
            target,
            &AddressPolicy::new(false),
        )
        .expect("builder")
        .build()
        .expect("client")
}

fn behind(proxy: u16, no_proxy: Option<&str>, lookup: Arc<Answers>) -> EnvironmentRoute {
    let endpoint = format!("http://127.0.0.1:{proxy}");
    EnvironmentRoute::new(
        EnvironmentProxy::given(Some(&endpoint), Some(&endpoint), no_proxy),
        lookup,
    )
}

#[test]
fn the_variables_are_read_per_scheme_with_their_login_and_no_proxy() {
    let proxies = EnvironmentProxy::given(
        Some("http://user:pa%3Ass@127.0.0.1:3128"),
        Some("socks5://socks:secret@proxy.test:1080"),
        Some("internal.example, 10.0.0.0/8"),
    );

    let (endpoint, login) = proxies
        .for_target(&url("http://files.test/file"))
        .expect("an http proxy");
    assert_eq!(endpoint.as_str(), "http://127.0.0.1:3128/");
    let login = login.expect("a login");
    assert_eq!(login.username, "user");
    assert_eq!(login.password.expose_secret(), "pa:ss");

    let (endpoint, login) = proxies
        .for_target(&url("https://files.test/file"))
        .expect("an https proxy");
    assert_eq!(endpoint.as_str(), "socks5://proxy.test:1080/");
    assert_eq!(login.expect("a login").username, "socks");

    for exempt in [
        "http://internal.example/hook",
        "https://ip.internal.example/",
        "http://10.1.2.3/",
        "ftp://files.test/file",
    ] {
        assert!(proxies.for_target(&url(exempt)).is_none(), "{exempt}");
    }
    assert!(
        EnvironmentProxy::given(None, None, None)
            .for_target(&url("http://files.test/"))
            .is_none()
    );
}

#[tokio::test]
async fn the_environment_proxy_is_handed_the_checked_address() {
    let (origin, seen) = plain_origin().await;
    let port = origin.addr().port();
    let (proxy, asked) = connect_proxy(None).await;
    let target = url(&format!("http://files.test:{port}/hook"));

    let body = fetch(
        &client(&behind(proxy, None, Answers::steady()), &target),
        target.as_str(),
    )
    .await;

    assert_eq!(body, "ok");
    assert_eq!(
        entries(&asked),
        [format!("CONNECT {PUBLIC}:{port} HTTP/1.1")]
    );
    assert_eq!(
        entries(&seen),
        [format!(
            "GET /hook host=files.test:{port} proxy-authorization=-"
        )]
    );
}

/// The acceptance case for the environment's proxy: the name passes the check with a public
/// address and answers with loopback when the connection is made.
#[tokio::test]
async fn a_name_that_answers_inside_after_the_check_never_reaches_the_environment_proxy() {
    let (proxy, asked) = connect_proxy(None).await;
    let route = behind(proxy, None, Answers::rebinding());
    let target = url("http://files.test:8080/hook");
    let checked = crate::check_target(&AddressPolicy::new(false), route.lookup(), &target)
        .await
        .expect("the check passes");
    assert_eq!(checked, [PUBLIC]);

    let error = client(&route, &target)
        .get(target.as_str())
        .send()
        .await
        .expect_err("refused");

    assert!(crate::is_refusal(&error), "{error:?}");
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}

/// `NO_PROXY` sends the target past the proxy, and still through the guarded resolver.
#[tokio::test]
async fn a_target_no_proxy_exempts_goes_direct_under_the_guard() {
    let (proxy, asked) = connect_proxy(None).await;
    let lookup = Answers::rebinding();
    let route = behind(proxy, Some("files.test"), Arc::clone(&lookup));
    let target = url("http://files.test:8080/hook");
    // The check's lookup; the connection's answers with loopback.
    lookup.lookup("files.test").await.expect("lookup");

    let error = client(&route, &target)
        .get(target.as_str())
        .send()
        .await
        .expect_err("refused");

    assert!(crate::is_refusal(&error), "{error:?}");
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}

/// A network where only the proxy resolves: the name is not handed over unchecked, the
/// request fails with `proxy.name_unresolved`.
#[tokio::test]
async fn a_name_only_the_proxy_could_resolve_fails_with_its_code() {
    let (proxy, asked) = connect_proxy(None).await;
    let target = url("http://intranet.test:8080/hook");

    let error = client(&behind(proxy, None, Answers::nothing()), &target)
        .get(target.as_str())
        .send()
        .await
        .expect_err("unresolved");

    assert!(!crate::is_refusal(&error), "{error:?}");
    assert_eq!(
        name_unresolved(&error).map(NameUnresolved::host),
        Some("intranet.test"),
        "{error:?}"
    );
    let failure = unresolved_failure(&error).expect("a coded failure");
    assert_eq!(failure.code.as_deref(), Some(CODE_NAME_UNRESOLVED));
    assert_eq!(
        failure.params.get("host").map(String::as_str),
        Some("intranet.test")
    );
    assert!(entries(&asked).is_empty(), "{:?}", entries(&asked));
}
