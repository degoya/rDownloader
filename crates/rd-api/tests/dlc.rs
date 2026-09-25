//! Integration tests for the DLC import surface.
//!
//! A DLC can only be decrypted by an online service, so the paths worth testing here are the
//! ones that must never reach it: the feature being off, a file that is not a container at
//! all, and an endpoint that could not be called in the first place. None of these tests
//! touches the network.

mod common;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{get_json, put_json, test_router};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Multipart body carrying one `.dlc` file.
fn multipart(boundary: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"release.dlc\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: application/x-dlc\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn import(router: &Router, content: &[u8]) -> (StatusCode, serde_json::Value) {
    let boundary = "rddlcboundary";
    let request = Request::post("/api/v1/dlc/import")
        .header(header::HOST, "127.0.0.1:8710")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(multipart(boundary, content)))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// Turns the DLC import on, leaving every other setting as stored.
async fn enable_dlc(router: &Router, endpoint: Option<&str>) -> (StatusCode, serde_json::Value) {
    let (status, mut settings) = get_json(router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "settings: {settings}");
    // Saving settings re-applies the login switch, which would otherwise turn the harness's
    // password-less session back on and answer the next request with `auth.setup_pending`.
    settings["admin_login_disabled"] = serde_json::Value::Bool(true);
    settings["dlc_service_enabled"] = serde_json::Value::Bool(true);
    settings["dlc_service_endpoint"] = endpoint.map_or(serde_json::Value::Null, |value| {
        serde_json::Value::String(value.to_owned())
    });
    put_json(router, "/api/v1/settings", settings).await
}

#[tokio::test]
async fn an_import_is_refused_while_the_feature_is_off() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;

    // A syntactically valid container: the refusal must come from the setting, not the parser.
    let container = "A".repeat(32) + &"B".repeat(88);
    let (status, body) = import(&router, container.as_bytes()).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["code"], "dlc.service_disabled");
    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    assert_eq!(settings["dlc_service_enabled"], false, "off by default");
}

#[tokio::test]
async fn a_file_that_is_not_a_container_is_rejected_before_any_request() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, body) = enable_dlc(&router, None).await;
    assert_eq!(status, StatusCode::OK, "settings: {body}");

    let (status, body) = import(&router, b"this is not a DLC container").await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["code"], "dlc.file_invalid");
}

#[tokio::test]
async fn an_endpoint_that_is_not_an_http_url_is_refused_when_it_is_saved() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = enable_dlc(&router, Some("file:///etc/passwd")).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["code"], "dlc.endpoint_invalid");
    // The rejected blob must not have been stored on the way.
    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    assert_eq!(settings["dlc_service_enabled"], false);
}

#[tokio::test]
async fn a_multipart_request_without_a_file_is_reported() {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = test_router(directory.path()).await;
    let boundary = "rddlcboundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nRelease\r\n--{boundary}--\r\n"
    );
    let request = Request::post("/api/v1/dlc/import")
        .header(header::HOST, "127.0.0.1:8710")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .expect("request");
    let response = router.oneshot(request).await.expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("error body");
    assert_eq!(payload["code"], "request.multipart_missing_file");
}
