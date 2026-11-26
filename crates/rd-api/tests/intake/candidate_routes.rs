//! The routes a LinkGrabber link and its package answer on when there is nothing to show or to
//! act on (audit 2026-11-25, TST-7). Their MCP tools are driven by the tool suite; what each route
//! refuses, and under which code the interface words it, is held here.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, request, test_router};
use serde_json::{Value, json};

/// A link without a listing, without media and without a mirror: each route says which is missing.
#[tokio::test]
async fn a_link_without_listing_media_or_mirror_is_refused_by_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = rd_core::CandidateId::new();

    let cases: [(&str, String, Option<Value>, &str); 4] = [
        (
            "GET",
            format!("/api/v1/collector/candidates/{id}/listing"),
            None,
            "remote.listing_missing",
        ),
        (
            "PUT",
            format!("/api/v1/collector/candidates/{id}/listing/plan"),
            Some(json!({ "excluded": ["nothing"] })),
            "remote.listing_missing",
        ),
        (
            "POST",
            format!("/api/v1/collector/candidates/{id}/media/output-preview"),
            Some(json!({ "template": "{title}.{ext}" })),
            "collector.candidate_missing",
        ),
        (
            "POST",
            format!("/api/v1/collector/candidates/{id}/mirror/dissolve"),
            Some(json!({})),
            "collector.mirror_not_grouped",
        ),
    ];
    for (method, uri, body, code) in cases {
        let (status, answer) = request(&router, method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {answer}");
        assert_eq!(answer["code"], code, "{method} {uri}: {answer}");
    }
}

/// A download whose trackers are not known yet is refused without a tracker being asked.
#[tokio::test]
async fn unknown_trackers_are_not_scraped() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let id = rd_core::DownloadId::new();
    let (status, answer) = common::post_json(
        &router,
        &format!("/api/v1/downloads/{id}/torrent/trackers/scrape"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "torrent.scrape_failed", "{answer}");
}

/// Lists that are empty because nothing is installed or imported answer an empty list.
#[tokio::test]
async fn lists_without_entries_are_empty_lists() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let import = rd_core::NzbImportId::new();
    for uri in [
        format!("/api/v1/nzb/imports/{import}/postprocess"),
        "/api/v1/postprocess/plugin-steps".to_owned(),
        "/api/v1/postprocess/upload-destinations".to_owned(),
    ] {
        let (status, answer) = get_json(&router, &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {answer}");
        assert_eq!(answer, json!([]), "{uri}");
    }
}
