//! The two capture mitigations that had no test of their own (security review 2026-09-28,
//! `docs/security/capture.md`): cross-origin access is open on the capture routes and nowhere
//! else, and a capture cannot choose a script, a category, a post-processing level or a
//! destination.

use crate::common;

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
};
use common::{API_BEARER, CAPTURE_BEARER, post_capture, request_to, send_raw, test_router};

const PAGE: &str = "https://page.example";

/// A CORS preflight from a web page for `method` on `uri`.
async fn preflight(router: &axum::Router, uri: &str, method: &str) -> HeaderMap {
    let request = request_to("OPTIONS", uri)
        .header(header::ORIGIN, PAGE)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type",
        )
        .body(Body::empty())
        .expect("request");
    send_raw(router, request).await.1
}

/// Capture routes answer a page's preflight and its request; the rest of the API, the login
/// and MCP answer neither with an `Access-Control-Allow-Origin`, so a browser keeps a foreign
/// page from reading them.
#[tokio::test]
async fn cross_origin_access_is_open_on_the_capture_routes_and_nowhere_else() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    for (uri, method) in [
        ("/api/v1/capture/batches", "POST"),
        ("/api/v1/capture/ping", "GET"),
    ] {
        let headers = preflight(&router, uri, method).await;
        assert_eq!(
            headers
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("*"),
            "{uri}: {headers:?}"
        );
    }
    let ping = request_to("GET", "/api/v1/capture/ping")
        .header(header::ORIGIN, PAGE)
        .header(header::AUTHORIZATION, format!("Bearer {CAPTURE_BEARER}"))
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = send_raw(&router, ping).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));

    for (uri, method) in [
        ("/api/v1/settings", "PUT"),
        ("/api/v1/collector/candidates", "GET"),
        ("/api/v1/auth/login", "POST"),
        ("/api/v1/auth/setup", "POST"),
        ("/mcp", "POST"),
    ] {
        let headers = preflight(&router, uri, method).await;
        assert!(
            !headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            "{uri} answers a foreign page's preflight: {headers:?}"
        );
    }
    let settings = request_to("GET", "/api/v1/settings")
        .header(header::ORIGIN, PAGE)
        .header(header::AUTHORIZATION, format!("Bearer {API_BEARER}"))
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = send_raw(&router, settings).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        "the settings answer is readable by a foreign page: {headers:?}"
    );
}

/// A capture proposes links for review; what happens to them afterwards is the person's
/// choice. Fields that would choose it are not part of the request and change nothing.
#[tokio::test]
async fn a_capture_cannot_choose_a_script_a_category_or_a_destination() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, payload) = post_capture(
        &router,
        serde_json::json!({
            "source": "browser_download",
            "source_label": "Chrome",
            "package_name": "release",
            "links": [{ "url": "https://files.example.com/release.bin" }],
            "script": "evil.sh",
            "category": "movies",
            "category_id": "00000000-0000-7000-8000-000000000001",
            "postprocess_level": "unpack_and_delete",
            "destination": "/tmp/rd-capture-elsewhere",
            "download_directory": "/tmp/rd-capture-elsewhere",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");

    let packages = payload["packages"].as_array().expect("packages");
    assert!(!packages.is_empty(), "{payload}");
    for package in packages {
        assert!(package["script"].is_null(), "{package}");
        assert!(package["category_id"].is_null(), "{package}");
        assert!(package["postprocess_level"].is_null(), "{package}");
    }
    for candidate in payload["candidates"].as_array().expect("candidates") {
        assert!(candidate["category_id"].is_null(), "{candidate}");
    }
    let serialized = payload.to_string();
    for chosen in [
        "evil.sh",
        "/tmp/rd-capture-elsewhere",
        "00000000-0000-7000-8000-000000000001",
    ] {
        assert!(
            !serialized.contains(chosen),
            "{chosen} was taken over: {payload}"
        );
    }
}
