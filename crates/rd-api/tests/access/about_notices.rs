//! The licence texts of what a package ships (RD-1480-12): read by a status page's token, by
//! nobody signed out.

use crate::common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{READ_BEARER, auth_harness, get_json, send_text};

const NOTICES: &str = "/api/v1/system/about/notices";

#[tokio::test]
async fn the_notices_answer_a_read_token_as_text_and_nobody_signed_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;

    let (status, body) = get_json(&harness.router, NOTICES).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "signed out: {body}");

    let request = Request::builder()
        .uri(NOTICES)
        .header(header::HOST, "localhost")
        .header(header::AUTHORIZATION, format!("Bearer {READ_BEARER}"))
        .body(Body::empty())
        .expect("request");
    let (status, content_type, text) = send_text(&harness.router, request).await;
    assert_eq!(status, StatusCode::OK, "{text:.200}");
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    assert!(text.contains("Permission is hereby granted"), "no MIT text");
    assert!(text.contains("Apache License"), "no Apache text");
}
