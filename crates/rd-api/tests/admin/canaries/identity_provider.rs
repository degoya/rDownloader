//! The identity provider's part of the canary run (RD-190-15, O-LEAK): the client secret planted
//! through the configuration route, and a link and a sign-in through a stand-in provider whose
//! token endpoint answers with planted access and refresh tokens. The ID tokens it minted join
//! the canaries afterwards, like the minted API token: none of them may come back out anywhere.

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
};
use serde_json::{Value, json};

use super::support::{Planted, Scan};
use crate::common::{
    self, Harness,
    idp::{CLIENT_ID, FakeIdp, Grant},
};

const SUBJECT: &str = "canary-administrator";

/// Configures the provider from `session`, links an identity and signs in through it.
pub(super) async fn sign_in_through_provider(
    harness: &Harness,
    session: &str,
    planted: &mut Planted,
    scan: &mut Scan,
) {
    let router = &harness.router;
    *harness.state.proxy.write().await = rd_authn::ProxyConfig::parse(
        &[],
        Some("https://canary.example.com"),
        rd_authn::CookieSecurity::Auto,
    )
    .expect("the external URL");
    let idp = FakeIdp::start_with(
        &planted.canary("oidc-client-secret"),
        &planted.canary("oidc-access-token"),
        &planted.canary("oidc-refresh-token"),
    )
    .await;
    let body = json!({
        "password": planted.canary("admin-password"),
        "issuer": idp.issuer,
        "client_id": CLIENT_ID,
        "client_secret": planted.canary("oidc-client-secret"),
        "display_name": "Canary ID",
    });
    let (status, answer) =
        common::put_json_with_cookie(router, "/api/v1/auth/oidc", session, body).await;
    assert_eq!(status, StatusCode::OK, "configure the provider: {answer}");
    scan.record(
        "PUT /api/v1/auth/oidc (answer)",
        answer.to_string().into_bytes(),
    );

    // The link: the route, the provider, the callback.
    let request = common::request_to("POST", "/api/v1/auth/oidc/link")
        .header(header::COOKIE, format!("rd_session={session}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "password": planted.canary("admin-password") }).to_string(),
        ))
        .expect("request");
    let (status, headers, bytes) = common::send_raw(router, request).await;
    assert_eq!(status, StatusCode::OK, "link");
    scan.record("POST /api/v1/auth/oidc/link (answer)", bytes.to_vec());
    let answer: Value = serde_json::from_slice(&bytes).expect("a JSON answer");
    let authorization = answer["authorization_url"]
        .as_str()
        .expect("an authorization URL")
        .to_owned();
    let headers = finish(harness, &idp, &authorization, &headers, "link", scan).await;
    assert!(location(&headers).ends_with("oidc=linked"), "{headers:?}");

    // The sign-in: the start, the provider, the callback.
    let request = common::request_to("GET", "/api/v1/auth/oidc/start")
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = common::send_raw(router, request).await;
    assert_eq!(status, StatusCode::FOUND, "start");
    let authorization = location(&headers);
    let headers = finish(harness, &idp, &authorization, &headers, "sign-in", scan).await;
    assert!(
        common::session_cookie(&headers).is_some(),
        "signed in through the provider: {headers:?}"
    );

    let minted = idp.minted();
    assert_eq!(minted.len(), 2, "one ID token per round trip");
    for token in minted {
        planted.canaries.push(("oidc-id-token", token));
    }
}

/// Plays the person at the provider and the browser coming back: the provider is told what the
/// code stands for, and the callback carries the binding the start or link handed out.
async fn finish(
    harness: &Harness,
    idp: &FakeIdp,
    authorization: &str,
    started: &HeaderMap,
    code: &str,
    scan: &mut Scan,
) -> HeaderMap {
    idp.expect_code(code, Grant::for_request(authorization, SUBJECT));
    let state = url::Url::parse(authorization)
        .expect("an authorization URL")
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("a state");
    let binding = started
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| value.strip_prefix("rd_oidc="))
        .and_then(|value| value.split(';').next())
        .expect("the binding cookie")
        .to_owned();
    let request = common::request_to(
        "GET",
        &format!("/api/v1/auth/oidc/callback?code={code}&state={state}"),
    )
    .header(header::COOKIE, format!("rd_oidc={binding}"))
    .body(Body::empty())
    .expect("request");
    let (status, headers, bytes) = common::send_raw(&harness.router, request).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "callback");
    let mut answer = format!("{headers:?}").into_bytes();
    answer.extend_from_slice(&bytes);
    scan.record(format!("GET callback ({code})"), answer);
    headers
}

fn location(headers: &HeaderMap) -> String {
    headers
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("a Location")
        .to_owned()
}
