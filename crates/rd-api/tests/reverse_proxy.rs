//! Running behind a reverse proxy: mount point, forwarded addresses and cookie flags.
//!
//! Every one of these fails silently when it is wrong. A base path that is not stripped gives
//! a 404 for the API and a blank page for the app; a forwarded header believed from the wrong
//! peer turns the client address into a string the client chooses; a `Secure` cookie on a
//! plain-HTTP deployment is dropped by the browser, so signing in appears to work and the next
//! request is not authenticated. None of them produce an error anyone would connect back to
//! the setting.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{auth_harness, test_harness};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Puts the service under `/downloads` by saving it as the external URL.
async fn mount_under(harness: &common::Harness, external_url: &str, proxies: &[&str]) {
    let (status, settings) = common::get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    let mut settings = settings;
    // The harness switches the login off directly on the service, not through the settings
    // document, so saving the document would switch it back on and every later request in
    // this test would answer 401 for a reason that has nothing to do with the mount point.
    settings["admin_login_disabled"] = serde_json::Value::Bool(true);
    settings["external_url"] = serde_json::Value::String(external_url.to_owned());
    settings["trusted_proxies"] = serde_json::Value::Array(
        proxies
            .iter()
            .map(|value| serde_json::Value::String((*value).to_owned()))
            .collect(),
    );
    let (status, body) = common::put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "saving the proxy settings: {body}");
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// The API answers under the mount point, and the scope policy still recognises the route.
///
/// The second half is the part that could have gone wrong quietly: nesting the router under
/// the base would have put `/downloads` into every `MatchedPath`, detaching the whole scope
/// table from the routes it describes without any test noticing.
#[tokio::test]
async fn the_api_answers_under_the_mount_point() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    mount_under(
        &harness,
        "http://rd.example.test/downloads",
        &["10.0.0.0/8"],
    )
    .await;

    let (status, body) = get(&harness.router, "/downloads/api/v1/downloads").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The SPA fallback also answers 200, so assert on the shape: an empty queue is `[]`, not
    // an HTML document.
    assert!(
        body.trim_start().starts_with('['),
        "the mount point was not stripped; got the app shell: {body}"
    );

    // The same route without the mount point still answers: a container health check and
    // anything else reaching the port directly must keep working.
    let (status, _) = get(&harness.router, "/api/v1/health").await;
    assert_eq!(status, StatusCode::OK);
}

/// A path that merely starts with the same letters is not under the mount point.
#[tokio::test]
async fn a_sibling_path_is_not_swallowed_by_the_mount_point() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    mount_under(&harness, "http://rd.example.test/dl", &[]).await;

    // `/dlc/import` is a real route and must not be read as `/dl` + `c/import`.
    let (status, _) = get(&harness.router, "/dlc/import").await;
    assert_ne!(status, StatusCode::NOT_FOUND);
}

/// The app shell is served under the mount point with its asset references rewritten.
#[tokio::test]
async fn the_application_shell_points_at_the_mount_point() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    mount_under(&harness, "http://rd.example.test/downloads", &[]).await;

    let (status, body) = get(&harness.router, "/downloads/queue").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("window.__RD_BASE__=\"/downloads\""),
        "the shell did not tell the application where it is mounted"
    );
    assert!(
        !body.contains("src=\"/assets/"),
        "an asset reference kept its root-relative path and would 404"
    );
}

/// The event stream is reachable under the mount point too.
///
/// Named in the job's acceptance criteria because it is the one an SPA silently loses: the
/// page loads, the queue renders once, and nothing ever updates again.
#[tokio::test]
async fn the_event_stream_is_reachable_under_the_mount_point() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    mount_under(&harness, "http://rd.example.test/downloads", &[]).await;

    let request = Request::builder()
        .method("GET")
        .uri("/downloads/api/v1/events")
        .header(header::HOST, "127.0.0.1:8710")
        .body(Body::empty())
        .expect("request");
    let response = harness
        .router
        .clone()
        .oneshot(request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
}

/// The MCP endpoint answers under the mount point.
#[tokio::test]
async fn the_mcp_endpoint_is_reachable_under_the_mount_point() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    mount_under(&harness, "http://rd.example.test/downloads", &[]).await;

    let (status, _) = get(&harness.router, "/downloads/mcp").await;
    // Whatever MCP answers a bare GET, it is not the SPA fallback and not a 404 from routing.
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "the mount point was not stripped"
    );
}

/// A configuration the service cannot act on is refused when it is saved.
#[tokio::test]
async fn an_unusable_proxy_configuration_is_refused_with_a_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (_, mut settings) = common::get_json(&harness.router, "/api/v1/settings").await;
    settings["trusted_proxies"] = serde_json::json!(["10.0.0.0/8", "this is not an address"]);
    let (status, body) = common::put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.proxy_invalid", "{body}");
    assert!(
        body["params"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("this is not an address"),
        "the refusal does not name the offending value: {body}"
    );
}

/// The session cookie carries `Secure` when the deployment is https, and not otherwise.
#[tokio::test]
async fn the_session_cookie_follows_the_external_scheme() {
    for (external, expect_secure) in [
        ("https://rd.example.test", true),
        ("http://rd.example.test", false),
    ] {
        let directory = tempfile::tempdir().expect("tempdir");
        let harness = auth_harness(directory.path()).await;
        // Setup happens before the login, and the settings route needs no session while the
        // password is unset only in the non-auth harness — so save the setting through the
        // authenticated one after signing in.
        let (status, body) = common::post_json(
            &harness.router,
            "/api/v1/auth/setup",
            serde_json::json!({ "password": "correct-horse-battery" }),
        )
        .await;
        assert!(status.is_success(), "{body}");
        let (_, _, token) = common::post_json_with_headers(
            &harness.router,
            "/api/v1/auth/login",
            serde_json::json!({ "password": "correct-horse-battery" }),
        )
        .await;
        let token = token.expect("a session");

        let (_, mut settings) =
            common::get_with_cookie(&harness.router, "/api/v1/settings", &token).await;
        settings["external_url"] = serde_json::Value::String(external.to_owned());
        let (status, body) =
            common::put_json_with_cookie(&harness.router, "/api/v1/settings", &token, settings)
                .await;
        assert_eq!(status, StatusCode::OK, "{external}: {body}");

        let (_, _, _) = common::post_json_with_headers(
            &harness.router,
            "/api/v1/auth/login",
            serde_json::json!({ "password": "correct-horse-battery" }),
        )
        .await;
        let cookie = common::login_cookie(&harness.router, "correct-horse-battery").await;
        assert_eq!(
            cookie.contains("; Secure"),
            expect_secure,
            "{external} produced `{cookie}`"
        );
    }
}
