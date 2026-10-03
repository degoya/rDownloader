//! Who reaches the service by which name, and whom a switched-off login trusts (security
//! review 2026-09-28, finding 3).
//!
//! DNS rebinding turns a page on the attacker's own domain into a same-origin client of this
//! service. Two defences meet here: a `Host` the service does not answer to is refused before
//! anything routes, and a switched-off administrator login lets in only a caller on this
//! machine — a LAN client, a container's port mapping or a proxy's audience has to sign in.

use std::net::SocketAddr;

use crate::common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode, header},
};
use common::{auth_harness, get_json, send, test_harness};
use serde_json::{Value, json};

const REFUSED: &str = "request.host_not_allowed";

fn named(method: &str, uri: &str, host: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, host)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .expect("request")
}

/// A request as the listener delivers it: with the peer's address attached.
fn from_peer(uri: &str, peer: [u8; 4], forwarded: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, common::HOST)
        .extension(ConnectInfo(SocketAddr::from((peer, 50_000))));
    if forwarded {
        builder = builder.header("x-forwarded-for", "203.0.113.9");
    }
    builder.body(Body::empty()).expect("request")
}

/// Saves the proxy fields of the settings document; the login stays switched off, as the
/// harness set it (see `reverse_proxy::mount_under`).
async fn save_hosts(
    harness: &common::Harness,
    external_url: Option<&str>,
    allowed: &[&str],
) -> (StatusCode, Value) {
    let (status, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["admin_login_disabled"] = Value::Bool(true);
    settings["external_url"] = external_url.map_or(Value::Null, |url| Value::String(url.into()));
    settings["allowed_hosts"] = json!(allowed);
    common::put_json(&harness.router, "/api/v1/settings", settings).await
}

/// The rebinding request: the attacker's own name, pointed at loopback. Without the check each
/// of these is answered — the API with the full scope set, since the login is off.
#[tokio::test]
async fn an_unknown_host_name_is_refused_on_every_surface() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for (method, uri) in [
        ("GET", "/api/v1/health"),
        ("GET", "/api/v1/settings"),
        ("GET", "/"),
        ("POST", "/mcp"),
        ("GET", "/sabnzbd/api?mode=version"),
    ] {
        let (status, body) =
            send(&harness.router, named(method, uri, "attacker.example:8710")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body}");
        assert_eq!(body["code"], REFUSED, "{method} {uri}");
        assert_eq!(body["params"]["host"], "attacker.example:8710");
    }
    // The absolute form names the host in the request line instead of the header.
    let absolute = Request::builder()
        .uri("http://attacker.example:8710/api/v1/settings")
        .body(Body::empty())
        .expect("request");
    let (status, body) = send(&harness.router, absolute).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], REFUSED);
}

/// A browser opening the service under a refused name gets a page with the way forward instead
/// of JSON (RD-190-17): the name, the setting that allows it and the address to change it from.
/// The name is what the caller sent, so markup in it arrives escaped.
#[tokio::test]
async fn a_browser_is_shown_the_refused_name_and_where_to_allow_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let accept = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

    let request = Request::builder()
        .uri("/")
        .header(header::HOST, "rd.example.com")
        .header(header::ACCEPT, accept)
        .body(Body::empty())
        .expect("request");
    let (status, content_type, page) = common::send_text(&harness.router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{page}");
    assert!(content_type.starts_with("text/html"), "{content_type}");
    assert!(page.contains("<code>rd.example.com</code>"), "{page}");
    assert!(
        page.contains("Settings → Security → Reverse proxy → Allowed host names"),
        "{page}"
    );
    assert!(page.contains("http://127.0.0.1:8710"), "{page}");
    assert!(page.contains(REFUSED), "{page}");
    assert!(
        !page.contains("<script"),
        "nothing of the interface's bundle: {page}"
    );

    let request = Request::builder()
        .uri("/")
        .header(header::HOST, "<script>alert(1)</script>.example")
        .header(header::ACCEPT, accept)
        .body(Body::empty())
        .expect("request");
    let (status, _, page) = common::send_text(&harness.router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{page}");
    assert!(!page.contains("<script>"), "{page}");
    assert!(
        page.contains("&lt;script&gt;alert(1)&lt;/script&gt;.example"),
        "{page}"
    );

    // A client that asks for JSON — the interface's own calls, every API client — keeps the
    // coded refusal.
    let request = Request::builder()
        .uri("/api/v1/health")
        .header(header::HOST, "rd.example.com")
        .header(header::ACCEPT, "application/json")
        .body(Body::empty())
        .expect("request");
    let (status, body) = send(&harness.router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], REFUSED);
    assert_eq!(body["params"]["host"], "rd.example.com");
}

/// Before the owner finishes setup, `/auth/setup` is public: a rebinding page must not be the
/// one that sets the password.
#[tokio::test]
async fn a_rebinding_page_cannot_claim_a_fresh_installation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let claim = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/setup")
        .header(header::HOST, "attacker.example")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "password": "the-attackers-own-password" }).to_string(),
        ))
        .expect("request");
    let (status, body) = send(&harness.router, claim).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], REFUSED);

    let (status, body) = get_json(&harness.router, "/api/v1/auth/status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["setup_required"], true, "the claim wrote nothing");
}

/// LAN access by address keeps working, and so does every spelling of loopback.
#[tokio::test]
async fn addresses_and_localhost_are_always_answered() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for host in [
        "127.0.0.1:8710",
        "[::1]:8710",
        "192.168.1.20:8710",
        "10.0.0.7",
        "localhost:8710",
        "LocalHost",
        "rdownloader.localhost:8710",
    ] {
        let (status, body) = send(&harness.router, named("GET", "/api/v1/health", host)).await;
        assert_eq!(status, StatusCode::OK, "{host}: {body}");
    }
}

#[tokio::test]
async fn the_external_url_host_and_the_listed_names_are_answered() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, _) = send(
        &harness.router,
        named("GET", "/api/v1/health", "nas.lan:8710"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not listed yet");

    let (status, body) = save_hosts(&harness, Some("https://rd.example.com"), &["NAS.lan"]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["allowed_hosts"], json!(["NAS.lan"]), "stored as typed");
    for host in ["rd.example.com", "rd.example.com:443", "nas.lan:8710"] {
        let (status, body) = send(&harness.router, named("GET", "/api/v1/health", host)).await;
        assert_eq!(status, StatusCode::OK, "{host}: {body}");
    }
    let (status, body) = send(&harness.router, named("GET", "/api/v1/health", "other.lan")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn an_allowed_host_that_is_no_host_name_is_refused_on_save() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = save_hosts(&harness, None, &["http://nas.lan:8710"]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.proxy_invalid");
}

/// The second half of the finding: with the login off, only this machine is an administrator.
/// Without the fix the LAN peer and the proxied request get the downloads list like loopback.
#[tokio::test]
async fn a_switched_off_login_lets_only_this_machine_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, body) = send(
        &harness.router,
        from_peer("/api/v1/downloads", [127, 0, 0, 1], false),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "loopback: {body}");

    for (peer, forwarded) in [
        ([192, 168, 1, 20], false),
        ([172, 17, 0, 1], false),
        ([127, 0, 0, 1], true),
    ] {
        let (status, body) = send(
            &harness.router,
            from_peer("/api/v1/downloads", peer, forwarded),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{peer:?} {forwarded}: {body}"
        );
        assert_eq!(body["code"], "auth.setup_pending");

        let (status, body) = send(
            &harness.router,
            from_peer("/api/v1/auth/status", peer, forwarded),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["login_disabled"], false, "{peer:?} {forwarded}");
        assert_eq!(body["authenticated"], false, "{peer:?} {forwarded}");
    }

    let (status, body) = send(
        &harness.router,
        from_peer("/api/v1/auth/status", [127, 0, 0, 1], false),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["login_disabled"], true);
}

/// The MCP transport gate had its own copy of the switch.
#[tokio::test]
async fn a_switched_off_login_does_not_open_mcp_to_another_machine() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::HOST, common::HOST)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .extension(ConnectInfo(SocketAddr::from(([192, 168, 1, 20], 50_000))))
        .body(Body::from(
            json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string(),
        ))
        .expect("request");
    let (status, _) = send(&harness.router, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The list decides who may reach the service, so changing it costs `api:admin` like the rest
/// of the proxy contract; a configuration token could otherwise let a rebinding name back in.
#[tokio::test]
async fn a_config_token_cannot_change_the_allowed_hosts() {
    const CONFIG_BEARER: &str = "test-config-bearer-host-check";
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(
        directory.path(),
        common::Options::default()
            .login()
            .token(CONFIG_BEARER, rd_core::API_CONFIG_SCOPE),
    )
    .await;
    let (status, mut settings) =
        common::get_with_bearer(&harness.router, "/api/v1/settings", CONFIG_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    settings["allowed_hosts"] = json!(["attacker.example"]);
    let request = common::request_to("PUT", "/api/v1/settings")
        .header(header::AUTHORIZATION, format!("Bearer {CONFIG_BEARER}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(settings.to_string()))
        .expect("request");
    let (status, body) = send(&harness.router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["params"]["setting"], "allowed_hosts");
}
