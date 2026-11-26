//! `Idempotency-Key` on `POST /api/v1/downloads` (RD-1480-10): a repeat is answered the download
//! the first request created, per credential and per body.

use crate::common;

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use common::{API_BEARER, Harness, Options, get_with_bearer, harness, parked_harness};
use serde_json::{Value, json};

const OTHER_BEARER: &str = "another-api-bearer-token";

/// `POST /api/v1/downloads` with `key`, as the holder of `bearer`.
async fn add(harness: &Harness, bearer: &str, key: &str, url: &str) -> (StatusCode, Value) {
    let request = common::request_to("POST", "/api/v1/downloads")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header("idempotency-key", key)
        .body(Body::from(
            json!({ "url": url, "paused": true }).to_string(),
        ))
        .expect("request");
    let (status, _, bytes) = common::send_raw(&harness.router, request).await;
    (status, serde_json::from_slice(&bytes).expect("json"))
}

async fn queued(harness: &Harness) -> usize {
    let (status, rows) = get_with_bearer(&harness.router, "/api/v1/downloads", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    rows.as_array().expect("rows").len()
}

/// Two bearers, each with the whole API.
async fn two_callers(directory: &std::path::Path) -> Harness {
    let options = Options::default()
        .login()
        .parked()
        .token(OTHER_BEARER, rd_core::API_SCOPE);
    harness(directory, options).await
}

#[tokio::test]
async fn a_repeat_is_answered_the_first_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = two_callers(directory.path()).await;
    let (status, first) = add(&harness, API_BEARER, "key-1", "http://example.test/a.bin").await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, again) = add(&harness, API_BEARER, "key-1", "http://example.test/a.bin").await;
    assert_eq!(status, StatusCode::CREATED, "{again}");
    assert_eq!(again["id"], first["id"]);
    assert_eq!(queued(&harness).await, 1);

    // Another key is another download, as every request without the header is.
    let (status, other) = add(&harness, API_BEARER, "key-2", "http://example.test/a.bin").await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    assert_ne!(other["id"], first["id"]);
    assert_eq!(queued(&harness).await, 2);
}

#[tokio::test]
async fn two_repeats_at_once_create_one_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = two_callers(directory.path()).await;
    let url = "http://example.test/a.bin";
    let (left, right) = tokio::join!(
        add(&harness, API_BEARER, "key-1", url),
        add(&harness, API_BEARER, "key-1", url)
    );
    assert_eq!(left.0, StatusCode::CREATED, "{}", left.1);
    assert_eq!(right.0, StatusCode::CREATED, "{}", right.1);
    assert_eq!(left.1["id"], right.1["id"]);
    assert_eq!(queued(&harness).await, 1);
}

#[tokio::test]
async fn another_body_under_the_same_key_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = two_callers(directory.path()).await;
    let (status, first) = add(&harness, API_BEARER, "key-1", "http://example.test/a.bin").await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, refused) = add(&harness, API_BEARER, "key-1", "http://example.test/b.bin").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(refused["code"], "request.idempotency_key_reused");
    assert_eq!(queued(&harness).await, 1);
}

#[tokio::test]
async fn a_key_belongs_to_the_credential_that_sent_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = two_callers(directory.path()).await;
    let (_, mine) = add(&harness, API_BEARER, "key-1", "http://example.test/a.bin").await;
    let (status, theirs) = add(&harness, OTHER_BEARER, "key-1", "http://example.test/b.bin").await;
    assert_eq!(status, StatusCode::CREATED, "{theirs}");
    assert_ne!(theirs["id"], mine["id"]);
    assert_eq!(queued(&harness).await, 2);
}

/// A request that created nothing leaves its key free for the corrected one.
#[tokio::test]
async fn a_refused_request_does_not_use_up_its_key() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = two_callers(directory.path()).await;
    let (status, refused) = add(&harness, API_BEARER, "key-1", "gopher://example.test/a").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, created) = add(&harness, API_BEARER, "key-1", "http://example.test/a.bin").await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}

/// A repeat after the download was removed says so instead of adding it again.
#[tokio::test]
async fn a_removed_download_is_not_created_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = two_callers(directory.path()).await;
    let (_, first) = add(&harness, API_BEARER, "key-1", "http://example.test/a.bin").await;
    let uri = format!("/api/v1/downloads/{}", first["id"].as_str().expect("id"));
    let (status, body) =
        common::request_with_bearer(&harness.router, "DELETE", &uri, API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, gone) = add(&harness, API_BEARER, "key-1", "http://example.test/a.bin").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{gone}");
    assert_eq!(gone["code"], "download.not_found");
    assert_eq!(queued(&harness).await, 0);
}

#[tokio::test]
async fn a_malformed_key_is_refused_before_anything_is_created() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let request = common::request_to("POST", "/api/v1/downloads")
        .header(header::CONTENT_TYPE, "application/json")
        .header("idempotency-key", "two words")
        .body(Body::from(
            json!({ "url": "http://example.test/a.bin", "paused": true }).to_string(),
        ))
        .expect("request");
    let (status, _, bytes) = common::send_raw(&harness.router, request).await;
    let body: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "request.idempotency_key_invalid");
    let (_, rows) = common::get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(rows.as_array().expect("rows").len(), 0);
}
