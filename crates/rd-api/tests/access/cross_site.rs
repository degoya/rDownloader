//! A page of another site cannot change anything through a browser on this machine (audit
//! 1.9.1, API-01).
//!
//! With the administrator login switched off every caller on this machine is the
//! administrator, the browser included -- and a browser sends a form `POST`, a `text/plain`
//! or a `multipart/form-data` body anywhere without asking first. The browser also says where
//! such a request came from, in `Origin` and `Sec-Fetch-Site`, which no page can set; those
//! are refused before any credential is consulted. The routes that read their body as raw
//! bytes additionally insist on their declared media type, which a page cannot send without a
//! preflight this service never answers.

use crate::common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{API_BEARER, send, test_harness};

const REFUSED: &str = "request.cross_site_refused";
const WRONG_TYPE: &str = "request.content_type_unsupported";

/// A request to `uri`, sent with these browser headers.
fn browser(
    method: &str,
    uri: &str,
    origin: Option<&str>,
    sec_fetch_site: Option<&str>,
    content_type: &str,
    body: &'static [u8],
) -> Request<Body> {
    let mut builder = common::request_to(method, uri).header(header::CONTENT_TYPE, content_type);
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    if let Some(site) = sec_fetch_site {
        builder = builder.header("sec-fetch-site", site);
    }
    builder.body(Body::from(body)).expect("request")
}

/// The attack the finding describes, and its siblings: each one would otherwise run with every
/// scope, because the harness has the login switched off.
#[tokio::test]
async fn a_foreign_page_cannot_change_anything_with_the_login_switched_off() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for (method, uri, content_type) in [
        (
            "POST",
            "/api/v1/plugins/install?trust_fingerprint=00",
            "text/plain",
        ),
        ("POST", "/api/v1/site-rules/import", "text/plain"),
        ("POST", "/api/v1/backups/restore/uploads", "text/plain"),
        (
            "POST",
            "/api/v1/nzb/imports",
            "multipart/form-data; boundary=x",
        ),
        ("POST", "/api/v1/downloads/bulk", "text/plain"),
    ] {
        for (origin, site) in [
            (Some("https://attacker.example"), Some("cross-site")),
            (Some("https://attacker.example"), None),
            // Same site to the browser, another application to this service.
            (Some("http://127.0.0.1:3000"), Some("same-site")),
            (Some("null"), None),
            // A browser that hides the origin still says where it came from.
            (None, Some("cross-site")),
        ] {
            let (status, body) = send(
                &harness.router,
                browser(method, uri, origin, site, content_type, b"x"),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{method} {uri} from {origin:?}/{site:?}: {body}"
            );
            assert_eq!(
                body["code"], REFUSED,
                "{method} {uri} from {origin:?}/{site:?}"
            );
        }
    }
}

/// The same refusal on `/mcp`, which the switched-off login opens to this machine as well --
/// on every method, as the MCP specification asks; the `GET` stream was let through
/// (RD-1190-22).
#[tokio::test]
async fn a_foreign_page_cannot_call_the_mcp_endpoint() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    for method in ["POST", "GET", "DELETE"] {
        let (status, body) = send(
            &harness.router,
            browser(
                method,
                "/mcp",
                Some("https://attacker.example"),
                Some("cross-site"),
                "text/plain",
                b"{}",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}: {body}");
        assert_eq!(body["code"], REFUSED, "{method}");
    }
}

/// The interface itself, a command-line client and a read are all left alone.
#[tokio::test]
async fn the_interface_a_script_and_a_read_still_pass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let settings = |origin: Option<&'static str>, site: Option<&'static str>| {
        browser(
            "POST",
            "/api/v1/site-rules/import",
            origin,
            site,
            "application/json",
            b"{\"format_version\":1,\"rules\":[]}",
        )
    };
    for (origin, site) in [
        // The interface, as a current browser sends it.
        (Some("http://127.0.0.1:8710"), Some("same-origin")),
        // An older browser: no Sec-Fetch-Site, the Origin matching the Host.
        (Some("http://127.0.0.1:8710"), None),
        // curl, a script, the updater.
        (None, None),
    ] {
        let (status, body) = send(&harness.router, settings(origin, site)).await;
        assert_eq!(status, StatusCode::OK, "{origin:?}/{site:?}: {body}");
    }
    // A read changes nothing, so where it came from does not matter here.
    let request = common::request_to("GET", "/api/v1/health")
        .header(header::ORIGIN, "https://attacker.example")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .expect("request");
    let (status, _) = send(&harness.router, request).await;
    assert_eq!(status, StatusCode::OK);
}

/// A bearer client is not a browser: it sends no browser headers, and the check leaves it be.
#[tokio::test]
async fn a_bearer_client_without_browser_headers_passes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let request = common::request_to("POST", "/api/v1/site-rules/import")
        .header(header::AUTHORIZATION, format!("Bearer {API_BEARER}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{\"format_version\":1,\"rules\":[]}"))
        .expect("request");
    let (status, body) = send(&harness.router, request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The raw-body routes take only their declared type, so a simple request never reaches them
/// even where no origin is sent (the second line behind the origin check).
#[tokio::test]
async fn a_raw_body_route_refuses_a_simple_request_s_media_type() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/backups/restore/uploads",
        serde_json::json!({}),
    )
    .await;
    assert!(status.is_success(), "{created}");
    let upload = created["id"].as_str().expect("upload id").to_owned();
    for (uri, simple) in [
        ("/api/v1/plugins/install".to_owned(), "text/plain"),
        (
            "/api/v1/plugins/preview".to_owned(),
            "application/x-www-form-urlencoded",
        ),
        ("/api/v1/site-rules/import".to_owned(), "text/plain"),
        (
            format!("/api/v1/backups/restore/uploads/{upload}?offset=0"),
            "multipart/form-data; boundary=x",
        ),
    ] {
        let method = if uri.contains("/restore/uploads/") {
            "PUT"
        } else {
            "POST"
        };
        let (status, body) = send(
            &harness.router,
            browser(method, &uri, None, None, simple, b"x"),
        )
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{uri}: {body}");
        assert_eq!(body["code"], WRONG_TYPE, "{uri}");
    }
}

/// A request without `Host` -- HTTP/2 may carry only `:authority` -- is compared against the
/// URI's authority, as `host_check` reads it, instead of being refused for naming no host
/// (audit 1.9.1, RA-API-05).
#[tokio::test]
async fn the_uri_authority_stands_in_for_a_missing_host() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let request = |origin: &str| {
        axum::http::Request::builder()
            .method("POST")
            .uri("http://127.0.0.1:8710/api/v1/site-rules/import")
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{\"format_version\":1,\"rules\":[]}"))
            .expect("request")
    };
    let (status, body) = send(&harness.router, request("http://127.0.0.1:8710")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(&harness.router, request("http://127.0.0.1:3000")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], REFUSED);
}

/// The value of `name` in `headers`, as text.
fn header_text<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// No page of another site can frame the interface, sniff an answer into another type or learn
/// the interface's address from a `Referer` (audit 2026-10-05, S5). The shell carries the full
/// content security policy, everything else the frame refusal.
#[tokio::test]
async fn every_answer_carries_the_security_headers() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    for uri in [
        "/",
        "/queue",
        "/favicon.svg",
        "/api/v1/health",
        "/api/v1/downloads",
    ] {
        let request = common::request_to("GET", uri)
            .header(header::AUTHORIZATION, format!("Bearer {API_BEARER}"))
            .body(Body::empty())
            .expect("request");
        let (status, headers, _) = common::send_raw(&harness.router, request).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(
            header_text(&headers, "x-content-type-options"),
            Some("nosniff"),
            "{uri}"
        );
        assert_eq!(
            header_text(&headers, "referrer-policy"),
            Some("same-origin"),
            "{uri}"
        );
        assert_eq!(
            header_text(&headers, "x-frame-options"),
            Some("DENY"),
            "{uri}"
        );
        let policy = header_text(&headers, "content-security-policy").unwrap_or_default();
        assert!(policy.contains("frame-ancestors 'none'"), "{uri}: {policy}");
        let shell = uri == "/" || uri == "/queue";
        assert_eq!(
            policy.contains("script-src 'self';"),
            shell,
            "{uri}: the shell, and only the shell, carries the full policy: {policy}"
        );
    }

    // An error answer carries them too.
    let request = common::request_to("GET", "/assets/does-not-exist.js")
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = common::send_raw(&harness.router, request).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(header_text(&headers, "x-frame-options"), Some("DENY"));

    // The host-refusal page keeps its own, stricter policy.
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/")
        .header(header::HOST, "attacker.example:8710")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .expect("request");
    let (status, headers, _) = common::send_raw(&harness.router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        header_text(&headers, "content-security-policy"),
        Some("default-src 'none'; style-src 'unsafe-inline'")
    );
    assert_eq!(header_text(&headers, "x-frame-options"), Some("DENY"));
}

/// The headers leave the capture routes' cross-origin answers alone: the browser extension
/// still reads them.
#[tokio::test]
async fn the_security_headers_leave_the_capture_cors_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let request = common::request_to("OPTIONS", "/api/v1/capture/batches")
        .header(header::ORIGIN, "chrome-extension://abcdefghijklmnop")
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type",
        )
        .body(Body::empty())
        .expect("request");
    let (_, headers, _) = common::send_raw(&harness.router, request).await;
    assert_eq!(
        header_text(&headers, "access-control-allow-origin"),
        Some("*")
    );
    assert_eq!(
        header_text(&headers, "x-content-type-options"),
        Some("nosniff")
    );
}
