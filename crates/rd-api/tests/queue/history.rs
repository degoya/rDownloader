//! The download history over REST (RD-1100-04): a package that completed and was removed is
//! found by its name, a failed one carries its code, "add again" puts the source back into the
//! LinkGrabber, the list pages in the database, and the clear needs its confirmation.
//!
//! The scheduler is parked and the lifecycle written by hand, as the other queue suites do.

use crate::common;

use axum::http::StatusCode;
use rd_core::{DownloadId, DownloadState, Failure, FailureKind, PackageId, PackageState};
use serde_json::json;

/// One package with one direct download; answers `(download id, package id)`.
async fn queued(harness: &common::Harness, package: &str, url: &str) -> (DownloadId, PackageId) {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": url, "package_name": package }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (
        created["id"]
            .as_str()
            .expect("id")
            .parse()
            .expect("download id"),
        created["package_id"]
            .as_str()
            .expect("package id")
            .parse()
            .expect("package id"),
    )
}

/// Finishes the download, completes its package and removes it from the queue.
async fn completed_and_removed(harness: &common::Harness, package: &str, url: &str) {
    let (download, package_id) = queued(harness, package, url).await;
    for state in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
    ] {
        harness
            .database
            .transition_download(download, state)
            .await
            .expect("transition");
    }
    harness
        .database
        .complete_download(download, "done.bin".to_owned(), None)
        .await
        .expect("complete");
    harness
        .database
        .set_package_state(package_id, PackageState::Completed, None, None, None)
        .await
        .expect("completed");
    harness
        .database
        .delete_download(download)
        .await
        .expect("removed");
}

/// Reads `uri` and answers its rows and its `X-Total-Count`, when it carries one.
async fn page(harness: &common::Harness, uri: &str) -> (serde_json::Value, Option<String>) {
    let (status, headers, bytes) = common::send_raw(
        &harness.router,
        common::request_to("GET", uri)
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    let total = headers
        .get("x-total-count")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    (serde_json::from_slice(&bytes).expect("json"), total)
}

#[tokio::test]
async fn a_removed_package_is_found_by_name_and_added_again_to_the_linkgrabber() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    completed_and_removed(
        &harness,
        "Holiday Pictures",
        "https://example.invalid/holiday.zip",
    )
    .await;
    completed_and_removed(&harness, "Other", "https://example.invalid/other.zip").await;

    let (rows, total) = page(&harness, "/api/v1/history?q=holiday&limit=10").await;
    assert_eq!(total.as_deref(), Some("1"));
    let entry = &rows[0];
    assert_eq!(entry["name"], "Holiday Pictures", "{rows}");
    assert_eq!(entry["outcome"], "completed", "{rows}");
    assert_eq!(entry["sources"][0], "https://example.invalid/holiday.zip");

    // Without a window the whole list comes back and no count header, like every API-15 list.
    let (rows, total) = page(&harness, "/api/v1/history").await;
    assert_eq!(rows.as_array().map(Vec::len), Some(2));
    assert_eq!(total, None);

    let id = entry["id"].as_i64().expect("id");
    let (status, added) = common::post_json(
        &harness.router,
        &format!("/api/v1/history/{id}/readd"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    let (_, candidates) = common::get_json(&harness.router, "/api/v1/collector/candidates").await;
    assert!(
        candidates
            .as_array()
            .expect("candidates")
            .iter()
            .any(|candidate| candidate["url"] == "https://example.invalid/holiday.zip"),
        "{candidates}"
    );

    let (status, missing) =
        common::post_json(&harness.router, "/api/v1/history/999999/readd", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert_eq!(missing["code"], "history.not_found");
}

#[tokio::test]
async fn a_failed_package_is_listed_with_its_code_and_the_filters_refuse_nonsense() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (download, _) = queued(&harness, "Broken", "https://example.invalid/broken.bin").await;
    harness
        .database
        .record_failure(
            download,
            Failure::coded(FailureKind::Offline, "http.not_found", "gone"),
            None,
        )
        .await
        .expect("failed");

    let (rows, total) = page(&harness, "/api/v1/history?outcome=failed&kind=http&limit=5").await;
    assert_eq!(total.as_deref(), Some("1"));
    assert_eq!(rows[0]["error_code"], "http.not_found", "{rows}");
    let (rows, _) = page(&harness, "/api/v1/history?outcome=completed").await;
    assert_eq!(rows.as_array().map(Vec::len), Some(0));

    for uri in [
        "/api/v1/history?outcome=maybe",
        "/api/v1/history?from=yesterday",
    ] {
        let (status, body) = common::get_json(&harness.router, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(body["code"], "history.filter_invalid", "{uri}");
    }
    let (status, body) = common::get_json(&harness.router, "/api/v1/history?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "request.page_limit");
}

#[tokio::test]
async fn the_clear_needs_its_confirmation_and_empties_only_the_history() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    completed_and_removed(&harness, "Gone", "https://example.invalid/gone.bin").await;
    let (kept, _) = queued(&harness, "Still queued", "https://example.invalid/kept.bin").await;

    let (status, refused) =
        common::post_json(&harness.router, "/api/v1/history/clear", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "data_reset.not_confirmed");

    let (status, cleared) = common::post_json(
        &harness.router,
        "/api/v1/history/clear",
        json!({ "confirmed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["removed"], 1);
    let (rows, _) = page(&harness, "/api/v1/history").await;
    assert_eq!(rows.as_array().map(Vec::len), Some(0));
    assert!(
        harness
            .database
            .get_download(kept)
            .await
            .expect("read")
            .is_some(),
        "the queue is untouched"
    );
}
