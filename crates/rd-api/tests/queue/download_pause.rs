//! Pausing one download through its own route (audit 2026-11-25, TST-7). The bulk route has its
//! suite (`bulk_pause`); this one is what the row's button calls.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, parked_harness, post_json};
use serde_json::json;

async fn state(router: &axum::Router, id: rd_core::DownloadId) -> serde_json::Value {
    let (status, downloads) = get_json(router, "/api/v1/downloads").await;
    assert_eq!(status, StatusCode::OK, "{downloads}");
    downloads
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == id.to_string()))
        .map(|row| row["state"].clone())
        .expect("the row")
}

#[tokio::test]
async fn a_queued_download_is_paused_and_resumed_by_its_own_routes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let id = crate::download_list::package_of(&harness, directory.path(), "One", 1).await[0];
    let pause = format!("/api/v1/downloads/{id}/pause");

    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/downloads/{id}/resume"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(state(&harness.router, id).await, "queued");

    // Twice: what is paused already stays paused, and asking again is no error.
    for _ in 0..2 {
        let (status, body) = post_json(&harness.router, &pause, json!({})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["code"], "download.paused", "{body}");
        assert_eq!(state(&harness.router, id).await, "paused");
    }
}

#[tokio::test]
async fn an_unknown_download_is_not_found_and_a_malformed_id_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;

    let unknown = rd_core::DownloadId::new();
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/downloads/{unknown}/pause"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "download.not_found", "{body}");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/nothing/pause",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "request.invalid_id", "{body}");
}
