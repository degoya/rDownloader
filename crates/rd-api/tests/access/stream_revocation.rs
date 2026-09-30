//! An open event stream ends when its credential stops standing (security audit 2026-09-30,
//! finding 4).
//!
//! A stream is authorised once, when it opens, and then stays open for hours. Signing out or
//! revoking a token used to leave it delivering the bus to a credential that no longer existed.
//! The harness re-checks every 100 ms instead of every 30 s, so these finish in a moment.

use std::time::Duration;

use crate::common;

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use common::{API_BEARER, CAPTURE_BEARER, Options, READ_BEARER, sign_in};
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSWORD: &str = "correct-horse-battery";
const RECHECK: Duration = Duration::from_millis(100);
/// Several re-checks' worth: a stream still open after this was not ended by a check.
const STAYS_OPEN: Duration = Duration::from_millis(500);
/// Generous, for a slow runner: the stream has to end within a few re-checks.
const ENDS_WITHIN: Duration = Duration::from_secs(10);

async fn harness(directory: &std::path::Path) -> common::Harness {
    common::harness(
        directory,
        Options::default().login().stream_recheck(RECHECK),
    )
    .await
}

/// Opens `uri` as an event stream with one credential header and hands back its body.
async fn open(router: &axum::Router, uri: &str, name: header::HeaderName, value: String) -> Body {
    let request = common::request_to("GET", uri)
        .header(name, value)
        .body(Body::empty())
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    response.into_body()
}

/// Whether the stream ends within `within`, reading and dropping every frame until then.
async fn ends_within(body: &mut Body, within: Duration) -> bool {
    tokio::time::timeout(within, async {
        while let Some(Ok(_)) = body.frame().await {}
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn signing_out_ends_the_sessions_stream() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let session = sign_in(&harness.router, PASSWORD).await;

    let mut body = open(
        &harness.router,
        "/api/v1/events",
        header::COOKIE,
        format!("rd_session={session}"),
    )
    .await;
    assert!(
        !ends_within(&mut body, STAYS_OPEN).await,
        "a stream with a live session was ended"
    );

    let (status, answer) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/auth/logout",
        &session,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(
        ends_within(&mut body, ENDS_WITHIN).await,
        "the stream outlived the sign-out"
    );
}

#[tokio::test]
async fn revoking_a_token_ends_its_stream() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;

    let mut body = open(
        &harness.router,
        "/api/v1/events",
        header::AUTHORIZATION,
        format!("Bearer {READ_BEARER}"),
    )
    .await;
    assert!(
        !ends_within(&mut body, STAYS_OPEN).await,
        "a stream with a live token was ended"
    );

    let (status, tokens) =
        common::get_with_bearer(&harness.router, "/api/v1/api-tokens", API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let id = tokens
        .as_array()
        .expect("a list")
        .iter()
        .find(|token| token["label"] == rd_core::API_READ_SCOPE)
        .and_then(|token| token["id"].as_str())
        .expect("the reading token")
        .to_owned();
    let (status, answer) = common::request_with_bearer(
        &harness.router,
        "DELETE",
        &format!("/api/v1/api-tokens/{id}"),
        API_BEARER,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");

    assert!(
        ends_within(&mut body, ENDS_WITHIN).await,
        "the stream outlived its token"
    );
}

#[tokio::test]
async fn revoking_a_capture_token_ends_the_capture_stream() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;

    let mut body = open(
        &harness.router,
        "/api/v1/capture/events",
        header::AUTHORIZATION,
        format!("Bearer {CAPTURE_BEARER}"),
    )
    .await;
    assert!(
        !ends_within(&mut body, STAYS_OPEN).await,
        "a capture stream with a live token was ended"
    );

    let capture = harness
        .database
        .list_capture_tokens(&[rd_core::CAPTURE_SCOPE])
        .await
        .expect("capture tokens");
    let capture = capture.first().expect("the harness mints one");
    harness
        .database
        .revoke_capture_token(capture.id)
        .await
        .expect("revoke");

    assert!(
        ends_within(&mut body, ENDS_WITHIN).await,
        "the capture stream outlived its token"
    );
}
