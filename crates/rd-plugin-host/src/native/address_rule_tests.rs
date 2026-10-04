//! Which addresses a plugin request may reach (RA-HOST-01, owner 2026-10-04): the service's
//! own listeners never, this machine's loopback and the person's own network only where they
//! entered the address, and an overlong token lifetime never panics (RA-HOST-02).

use std::sync::Arc;

use rd_http::{AddressPolicy, ClientPool, NetworkDefaults};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, ResolverHost};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::RwLock,
};
use url::Url;

use super::{NativeHost, address_policy, check_reach, token_expiry, with_own_network};
use crate::OwnEndpoints;

/// The service as if it listened on `127.0.0.1:8710`.
fn own() -> OwnEndpoints {
    OwnEndpoints::new(Some("127.0.0.1:8710".parse().expect("address")))
}

async fn test_host(dir: &std::path::Path) -> NativeHost {
    let database = rd_db::Database::open(dir.join("db.sqlite"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(dir.join("secrets"))
        .await
        .expect("secret store");
    NativeHost::new(
        database,
        ClientPool::default(),
        secrets,
        Arc::new(RwLock::new(NetworkDefaults::default())),
        None,
    )
    .with_own_endpoints(own())
}

fn get(url: &str) -> HostHttpRequest {
    HostHttpRequest {
        method: "GET".to_owned(),
        url: url.parse().expect("url"),
        query: Vec::new(),
        headers: Vec::new(),
        body: Vec::new(),
        granted_secret: None,
        // The manifest's own list, already applied by the sandbox; the provider registry plays
        // no part in which addresses are reachable.
        authority: rd_plugin_api::RequestAuthority::Manifest,
        write_methods: false,
    }
}

fn anonymous() -> ClientIdentity {
    ClientIdentity {
        account_id: None,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

/// A loopback server answering every request with `answer`; its port.
async fn loopback_server(answer: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = [0_u8; 2048];
            let _ = stream.read(&mut buffer).await;
            let _ = stream.write_all(answer.as_bytes()).await;
        }
    });
    port
}

const OK: &str = "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok";

/// A domain the plugin named reaches public addresses only: a manifest that listed `localhost`
/// reached the service's own API from this machine, which the API trusts, and
/// `169.254.169.254` is a cloud's metadata endpoint.
#[tokio::test]
async fn a_plugin_named_target_never_reaches_this_machine() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = test_host(dir.path()).await;
    let port = loopback_server(OK).await;
    for target in [
        format!("http://127.0.0.1:{port}/"),
        format!("http://localhost:{port}/"),
        format!("http://[::1]:{port}/"),
        format!("http://[::ffff:127.0.0.1]:{port}/"),
        "http://169.254.169.254/latest/meta-data/".to_owned(),
        "http://0.0.0.0:9/".to_owned(),
    ] {
        let refused = with_own_network(false, host.http_request(&anonymous(), get(&target)))
            .await
            .expect_err("this machine is not a plugin's to name");
        assert_eq!(
            refused.code.as_deref(),
            Some("plugin.http_local_target"),
            "{target}: {refused:?}"
        );
    }
}

/// An address the person entered may name a server beside the service — on loopback, on a
/// port that is not one of ours.
#[tokio::test]
async fn an_entered_address_reaches_loopback_on_another_port() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = test_host(dir.path()).await;
    let port = loopback_server(OK).await;
    for target in [
        format!("http://127.0.0.1:{port}/alerts"),
        format!("http://localhost:{port}/alerts"),
    ] {
        let answer = with_own_network(true, host.http_request(&anonymous(), get(&target)))
            .await
            .unwrap_or_else(|failure| panic!("{target}: {failure:?}"));
        assert_eq!(answer.status, 200, "{target}");
        assert_eq!(answer.body, b"ok");
    }
    // Link-local stays refused, entered or not.
    let refused = with_own_network(
        true,
        host.http_request(
            &anonymous(),
            get("http://169.254.169.254/latest/meta-data/"),
        ),
    )
    .await
    .expect_err("the metadata endpoint");
    assert_eq!(refused.code.as_deref(), Some("plugin.http_local_target"));
}

/// The service's own ports are no target, whoever entered the address: its API on the listen
/// port, the capture agent's Click'n'Load on 9666 — by literal, by name, and by redirect.
#[tokio::test]
async fn our_own_services_are_refused_even_for_an_entered_address() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = test_host(dir.path()).await;
    for target in [
        "http://127.0.0.1:8710/api/v1/settings",
        "http://localhost:8710/api/v1/settings",
        "http://[::1]:8710/api/v1/settings",
        "http://127.0.0.1:9666/flash/add",
        "http://localhost:9666/jdcheck.js",
    ] {
        for entered in [true, false] {
            let refused = with_own_network(entered, host.http_request(&anonymous(), get(target)))
                .await
                .expect_err("one of our own services");
            assert_eq!(
                refused.code.as_deref(),
                Some("plugin.http_own_service"),
                "{target}, entered {entered}: {refused:?}"
            );
        }
    }
    // A server beside the service may not send the request on to the service.
    let port = loopback_server(
        "HTTP/1.1 302 Found\r\nlocation: http://localhost:8710/api/v1/settings\r\n\
         content-length: 0\r\nconnection: close\r\n\r\n",
    )
    .await;
    let refused = with_own_network(
        true,
        host.http_request(&anonymous(), get(&format!("http://localhost:{port}/"))),
    )
    .await
    .expect_err("the redirect to our own port");
    assert_eq!(refused.code.as_deref(), Some("plugin.http_local_target"));
}

/// A literal, and any target behind a proxy, is judged before the request.
#[tokio::test]
async fn literals_and_proxied_targets_are_judged_before_the_request() {
    let url = |text: &str| -> Url { text.parse().expect("url") };
    let own = own();
    let public = AddressPolicy::new(false);
    let lan = AddressPolicy::new(true);
    for target in [
        "http://192.168.1.10/remote.php/dav/",
        "http://10.0.0.5:8080/",
        "http://[fd12:3456::1]/",
    ] {
        let refused = check_reach(&public, &url(target), false, &own)
            .await
            .expect_err("a domain the plugin named reaches public addresses only");
        assert_eq!(refused.code.as_deref(), Some("plugin.http_local_target"));
        assert!(check_reach(&lan, &url(target), false, &own).await.is_ok());
    }
    assert!(
        check_reach(&public, &url("http://93.184.216.34/"), false, &own)
            .await
            .is_ok()
    );
    // Through a proxy a name is judged here as well, since the proxy resolves it; one this
    // machine cannot resolve is left to the proxy.
    assert!(
        check_reach(&public, &url("http://localhost:9/"), true, &own)
            .await
            .is_err()
    );
    assert!(
        check_reach(&public, &url("http://unresolvable.invalid/"), true, &own)
            .await
            .is_ok()
    );
}

/// The invocation and the port decide the rule; a request outside any invocation gets the
/// public one.
#[tokio::test]
async fn the_rule_comes_from_the_invocation_and_the_port() {
    let own = own();
    let url = |text: &str| -> Url { text.parse().expect("url") };
    let ports = own.ports().to_vec();
    let public = AddressPolicy::new(false).refusing_redirects_to(&ports);
    assert_eq!(address_policy(&own, &url("http://localhost:2586/")), public);
    assert_eq!(
        with_own_network(true, async {
            address_policy(&own, &url("http://localhost:2586/"))
        })
        .await,
        AddressPolicy::new(true)
            .refusing_redirects_to(&ports)
            .with_loopback()
    );
    assert_eq!(
        with_own_network(true, async {
            address_policy(&own, &url("http://localhost:8710/"))
        })
        .await,
        AddressPolicy::new(true)
            .refusing_redirects_to(&ports)
            .listening_on(own.listen())
    );
}

/// RA-HOST-02: `expires-in` is the plugin's number, and `now + u64::MAX` seconds panicked in
/// `chrono`. It is held to a year.
#[test]
fn an_overlong_token_lifetime_is_held_to_a_year() {
    let now = chrono::Utc::now();
    let year = chrono::Duration::seconds(365 * 24 * 60 * 60);
    assert_eq!(token_expiry(now, u64::MAX), now + year);
    assert_eq!(token_expiry(now, i64::MAX.unsigned_abs()), now + year);
    assert_eq!(
        token_expiry(now, 3600),
        now + chrono::Duration::seconds(3600)
    );
    assert_eq!(token_expiry(now, 0), now);
}
