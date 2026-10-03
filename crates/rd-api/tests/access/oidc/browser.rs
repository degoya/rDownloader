//! The browser's side of a sign-in through the identity provider, and the installation it signs
//! in to: an administrator password, an external URL, and the provider configured.

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, StatusCode, header, request::Builder},
};
use serde_json::{Value, json};

use crate::common::{
    self, Harness,
    idp::{CLIENT_ID, FakeIdp, Grant},
};

pub const PASSWORD: &str = "correct-horse-battery";
pub const SECRET: &str = "oidc-client-secret-for-the-tests";
pub const EXTERNAL_URL: &str = "https://dl.example.com";
pub const CALLBACK: &str = "https://dl.example.com/api/v1/auth/oidc/callback";
/// The subject the administrator's account has at the provider.
pub const ADMINISTRATOR: &str = "administrator-subject";

/// A request from the browser at `203.0.113.<peer>`. Each test's refusals come from addresses of
/// their own, so the sign-in limiter, which counts them, never locks out the steps that follow.
pub fn from_browser(method: &str, uri: &str, peer: u8) -> Builder {
    common::request_to(method, uri)
        .extension(ConnectInfo(SocketAddr::from(([203, 0, 113, peer], 50_000))))
}

/// An installation with a password, an external URL and a session, and a provider beside it.
pub struct World {
    pub harness: Harness,
    pub idp: FakeIdp,
    /// A session opened with the password.
    pub session: String,
    pub _directory: tempfile::TempDir,
}

/// The installation, with or without the external URL the provider needs.
pub async fn world_with(options: common::Options, external_url: bool) -> World {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(directory.path(), options.login()).await;
    if external_url {
        *harness.state.proxy.write().await =
            rd_authn::ProxyConfig::parse(&[], Some(EXTERNAL_URL), rd_authn::CookieSecurity::Auto)
                .expect("the external URL");
    }
    let session = common::sign_in(&harness.router, PASSWORD).await;
    let idp = FakeIdp::start(SECRET).await;
    World {
        harness,
        idp,
        session,
        _directory: directory,
    }
}

/// The installation with the provider configured and the administrator's identity linked.
pub async fn linked_world() -> World {
    let world = world_with(common::Options::default(), true).await;
    let (status, body) = configure(&world, json!({})).await;
    assert_eq!(status, StatusCode::OK, "configure: {body}");
    let (status, headers) = link(&world, ADMINISTRATOR, 1).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/settings/security?oidc=linked");
    world
}

/// `PUT /api/v1/auth/oidc` from the password session, with `extra` laid over the usual fields.
pub async fn configure(world: &World, extra: Value) -> (StatusCode, Value) {
    let mut body = json!({
        "password": PASSWORD,
        "issuer": world.idp.issuer,
        "client_id": CLIENT_ID,
        "client_secret": SECRET,
        "display_name": "Pocket ID",
    });
    if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
        for (name, value) in source {
            target.insert(name.clone(), value.clone());
        }
    }
    common::put_json_with_cookie(
        &world.harness.router,
        "/api/v1/auth/oidc",
        &world.session,
        body,
    )
    .await
}

/// Links the account `subject` at the provider from the password session: the link route, the
/// provider, the callback.
pub async fn link(world: &World, subject: &str, peer: u8) -> (StatusCode, HeaderMap) {
    let (status, headers, body) = common::send_raw(
        &world.harness.router,
        from_browser("POST", "/api/v1/auth/oidc/link", peer)
            .header(header::COOKIE, format!("rd_session={}", world.session))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "password": PASSWORD }).to_string()))
            .expect("request"),
    )
    .await;
    let body: Value = serde_json::from_slice(&body).expect("a JSON answer");
    assert_eq!(status, StatusCode::OK, "link: {body}");
    let authorization = body["authorization_url"]
        .as_str()
        .expect("an authorization URL")
        .to_owned();
    let binding = cookie(&headers, "rd_oidc").expect("the binding cookie");
    world
        .idp
        .expect_code("link-code", Grant::for_request(&authorization, subject));
    callback(
        world,
        &parameter(&authorization, "state").expect("a state"),
        "link-code",
        Some(&binding),
        peer,
    )
    .await
}

/// A started sign-in: where the browser was sent, and the binding it holds.
pub struct Started {
    pub authorization: String,
    pub binding: String,
    pub state: String,
}

/// `GET /api/v1/auth/oidc/start`, which has to send the browser to the provider.
pub async fn start(world: &World, return_to: Option<&str>, peer: u8) -> Started {
    let (status, headers) = start_raw(world, return_to, peer, &[]).await;
    assert_eq!(status, StatusCode::FOUND, "{headers:?}");
    let authorization = location(&headers);
    assert!(
        authorization.starts_with(&format!("{}/authorize?", world.idp.issuer)),
        "{authorization}"
    );
    Started {
        binding: cookie(&headers, "rd_oidc").expect("the binding cookie"),
        state: parameter(&authorization, "state").expect("a state"),
        authorization,
    }
}

/// The start as it answered, with `extra` headers on the request.
pub async fn start_raw(
    world: &World,
    return_to: Option<&str>,
    peer: u8,
    extra: &[(&str, &str)],
) -> (StatusCode, HeaderMap) {
    let uri = match return_to {
        Some(path) => format!(
            "/api/v1/auth/oidc/start?return_to={}",
            url::form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>()
        ),
        None => "/api/v1/auth/oidc/start".to_owned(),
    };
    let mut request = from_browser("GET", &uri, peer);
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    let (status, headers, _) = common::send_raw(
        &world.harness.router,
        request.body(Body::empty()).expect("request"),
    )
    .await;
    (status, headers)
}

/// The provider's redirect back, carrying `binding` as the `rd_oidc` cookie when given.
pub async fn callback(
    world: &World,
    state: &str,
    code: &str,
    binding: Option<&str>,
    peer: u8,
) -> (StatusCode, HeaderMap) {
    callback_with(world, &format!("code={code}&state={state}"), binding, peer).await
}

/// The callback with a query of the test's own making.
pub async fn callback_with(
    world: &World,
    query: &str,
    binding: Option<&str>,
    peer: u8,
) -> (StatusCode, HeaderMap) {
    let mut request = from_browser("GET", &format!("/api/v1/auth/oidc/callback?{query}"), peer);
    if let Some(binding) = binding {
        request = request.header(header::COOKIE, format!("rd_oidc={binding}"));
    }
    let (status, headers, _) = common::send_raw(
        &world.harness.router,
        request.body(Body::empty()).expect("request"),
    )
    .await;
    (status, headers)
}

/// A whole sign-in through the provider as `grant` says, returning the callback's answer.
pub async fn sign_in_as(
    world: &World,
    code: &str,
    grant: impl FnOnce(Grant) -> Grant,
    peer: u8,
) -> (StatusCode, HeaderMap) {
    let started = start(world, None, peer).await;
    world.idp.expect_code(
        code,
        grant(Grant::for_request(&started.authorization, ADMINISTRATOR)),
    );
    callback(world, &started.state, code, Some(&started.binding), peer).await
}

/// A sign-in through the provider that has to succeed, returning the session it opened.
pub async fn provider_session(world: &World, code: &str, peer: u8) -> String {
    let (status, headers) = sign_in_as(world, code, |grant| grant, peer).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(location(&headers), "/", "{headers:?}");
    common::session_cookie(&headers).expect("a session cookie")
}

pub fn location(headers: &HeaderMap) -> String {
    headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("a Location")
        .to_owned()
}

/// The stable code a refused callback or start sent the browser back with.
pub fn refusal(headers: &HeaderMap) -> Option<String> {
    parameter(&location(headers), "oidc_error")
}

/// One query parameter of an absolute or relative URL.
pub fn parameter(address: &str, name: &str) -> Option<String> {
    let url = url::Url::parse("http://relative.invalid")
        .and_then(|base| base.join(address))
        .expect("a URL");
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

/// The value of the cookie `name` among the answer's `Set-Cookie` headers.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| value.strip_prefix(&format!("{name}=")))
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Every audit record of `action`, newest first.
pub async fn audited(world: &World, action: rd_core::AuditAction) -> Vec<rd_db::AuditRecord> {
    world
        .harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(action),
            limit: 200,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("the audit log")
}
