//! A known route asked for a method it does not have (RD-1430-04): `405` with `Allow` and a
//! stable code, the same with and without a sign-in, and never the scope refusal a typo in the
//! method used to read as. A path no route has is a `404`, and a malformed request a refusal, in the same
//! format (RD-1480-08).

use crate::common;

use axum::{
    body::Body,
    http::{HeaderMap, StatusCode, header},
};
use common::{auth_harness, request_to, send_raw, sign_in};

const PASSWORD: &str = "correct-horse-battery";

/// Sends `method` to `uri`, with a session cookie when `token` is given.
async fn call(
    harness: &common::Harness,
    method: &str,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, HeaderMap, serde_json::Value) {
    let mut builder = request_to(method, uri);
    if let Some(token) = token {
        builder = builder.header(header::COOKIE, format!("rd_session={token}"));
    }
    let request = builder.body(Body::empty()).expect("request");
    let (status, headers, bytes) = send_raw(&harness.router, request).await;
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, headers, body)
}

/// The methods an `Allow` header names, upper-case and sorted.
fn allowed(headers: &HeaderMap) -> Vec<String> {
    let mut methods: Vec<String> = headers
        .get(header::ALLOW)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(',')
        .map(|method| method.trim().to_ascii_uppercase())
        .filter(|method| !method.is_empty())
        .collect();
    methods.sort_unstable();
    methods
}

/// A protected route and a public one, each asked for a method it lacks, signed in and not.
#[tokio::test]
async fn a_wrong_method_answers_405_with_allow_signed_in_or_not() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router, PASSWORD).await;

    // `/api/v1/queue/search` has `GET` only (protected); `/api/v1/health` too (public).
    for uri in ["/api/v1/queue/search", "/api/v1/health"] {
        for session in [None, Some(token.as_str())] {
            let (status, headers, body) = call(&harness, "DELETE", uri, session).await;
            let signed_in = session.is_some();
            assert_eq!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "DELETE {uri}, signed in: {signed_in}: {body}"
            );
            assert_eq!(
                body["code"], "request.method_not_allowed",
                "DELETE {uri}, signed in: {signed_in}: {body}"
            );
            assert_eq!(
                allowed(&headers),
                ["GET", "HEAD"],
                "DELETE {uri}, signed in: {signed_in}"
            );
        }
    }

    // A route with several methods names all of them.
    let (status, headers, body) = call(&harness, "POST", "/api/v1/queue/pause", Some(&token)).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{body}");
    assert_eq!(allowed(&headers), ["DELETE", "GET", "HEAD", "PUT"]);
}

/// A path no route has is a coded `404` for a read too, never the interface's page (RD-1480-08);
/// the interface's own pages still answer.
#[tokio::test]
async fn an_unknown_api_path_is_a_json_404_signed_in_or_not() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let token = sign_in(&harness.router, PASSWORD).await;

    for session in [None, Some(token.as_str())] {
        for method in ["GET", "POST"] {
            let (status, headers, body) =
                call(&harness, method, "/api/v1/no-such-route", session).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method}: {body}");
            assert_eq!(body["code"], "web.route_not_found", "{method}: {body}");
            assert_eq!(
                headers
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("application/json"),
                "{method}"
            );
        }
    }
}

/// A malformed request to a real route is answered in the one error format, with the status it
/// always had (RD-1480-08).
#[tokio::test]
async fn a_malformed_request_is_answered_with_a_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;

    for (method, uri, body, status, code) in [
        (
            "POST",
            "/api/v1/downloads",
            Some("{"),
            StatusCode::BAD_REQUEST,
            "request.body_invalid",
        ),
        (
            "POST",
            "/api/v1/downloads",
            Some("[]"),
            StatusCode::UNPROCESSABLE_ENTITY,
            "request.body_invalid",
        ),
        (
            "POST",
            "/api/v1/downloads",
            None,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "request.content_type_unsupported",
        ),
        (
            "GET",
            "/api/v1/downloads/not-an-id/torrent/peers",
            None,
            StatusCode::BAD_REQUEST,
            "request.invalid_id",
        ),
        (
            "GET",
            "/api/v1/audit/records?limit=many",
            None,
            StatusCode::BAD_REQUEST,
            "request.query_invalid",
        ),
    ] {
        let mut request = request_to(method, uri);
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let request = request
            .body(Body::from(body.unwrap_or_default()))
            .expect("request");
        let (answered, _, bytes) = send_raw(&harness.router, request).await;
        let refusal: serde_json::Value =
            serde_json::from_slice(&bytes).expect("the answer is JSON");
        assert_eq!(answered, status, "{method} {uri}: {refusal}");
        assert_eq!(refusal["code"], code, "{method} {uri}: {refusal}");
    }
}

/// The method the route has still goes through the credential and the scope check.
#[tokio::test]
async fn the_right_method_still_needs_its_credential() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    sign_in(&harness.router, PASSWORD).await;

    let (status, _, body) = call(&harness, "GET", "/api/v1/queue/pause", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "auth.session_required", "{body}");
}
