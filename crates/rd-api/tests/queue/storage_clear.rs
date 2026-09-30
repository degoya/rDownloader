//! Clearing the storage history and the content index over REST (RD-180-13).
//!
//! What a client sees: the count arrives before the question, the history keeps an operation
//! still running, an emptied index forgets content duplicates until "Check index" puts back
//! what it finds through the rows, both clears are audited with their count, and both cost
//! `api:admin` while the check beside them stays queue work. The confirmation refusal and the
//! "the queue survives every clear" check hold for these two as well, through the list in
//! `crates/rd-api/tests/admin/data_reset.rs`.

use crate::collisions::{SOURCE, finished, paused_download};
use crate::common;

use axum::http::StatusCode;
use rd_core::{StorageOperationKind, StorageOperationState};
use rd_db::{NewStorageOperation, StorageOperationOutcome};
use serde_json::json;

const QUEUE_BEARER: &str = "storage-clear-queue-bearer";
const ADMIN_BEARER: &str = "storage-clear-admin-bearer";

async fn preview(router: &axum::Router) -> serde_json::Value {
    let (status, body) = common::get_json(router, "/api/v1/system/data-reset").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The one audit entry `action` left, which says how many rows went.
async fn audited_count(router: &axum::Router, action: &str) -> serde_json::Value {
    let (status, records) = common::get_json(
        router,
        &format!("/api/v1/audit/records?action={action}&limit=50"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{records}");
    let rows = records["records"].as_array().expect("records");
    assert_eq!(rows.len(), 1, "the clear is in the audit log: {records}");
    assert_eq!(rows[0]["outcome"], "success", "{records}");
    rows[0]["details"][rd_db::CLEARED_DETAIL_KEY].clone()
}

fn operation() -> NewStorageOperation {
    NewStorageOperation {
        kind: StorageOperationKind::Move,
        package_id: None,
        download_id: None,
        source_path: "/old/file.bin".to_owned(),
        target_path: "/new/file.bin".to_owned(),
        size_bytes: Some(3),
    }
}

#[tokio::test]
async fn clearing_the_history_keeps_a_running_operation_and_audits_itself() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let finished_id = harness
        .database
        .start_storage_operation(operation())
        .await
        .expect("start");
    harness
        .database
        .finish_storage_operation(
            finished_id,
            StorageOperationOutcome::completed(Some(3), Some("abc".to_owned())),
        )
        .await
        .expect("finish");
    let running = harness
        .database
        .start_storage_operation(operation())
        .await
        .expect("start");
    assert_eq!(
        preview(&harness.router).await["storage_operations"],
        1,
        "the count names what goes, not the running row"
    );

    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/storage/operations/clear",
        json!({ "confirmed": true }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 1, "{body}");
    let (status, history) = common::get_json(&harness.router, "/api/v1/storage/operations").await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let rows = history.as_array().expect("history");
    assert_eq!(rows.len(), 1, "{history}");
    assert_eq!(rows[0]["id"], running, "{history}");
    assert_eq!(rows[0]["state"], "running", "{history}");
    assert_eq!(preview(&harness.router).await["storage_operations"], 0);
    assert_eq!(
        audited_count(&harness.router, "storage_history_cleared").await,
        "1"
    );

    // The row that stayed still takes the outcome of its operation.
    harness
        .database
        .finish_storage_operation(
            running,
            StorageOperationOutcome::failed("storage.move_failed", "late".to_owned()),
        )
        .await
        .expect("finish");
    let history = harness
        .database
        .list_storage_operations(10)
        .await
        .expect("list");
    assert_eq!(history[0].state, StorageOperationState::Failed);
}

/// The consequence the dialog names: an emptied index knows no content duplicate and refuses a
/// link, and the check puts back what it finds through the finished rows.
#[tokio::test]
async fn an_emptied_index_forgets_duplicates_until_the_check_puts_them_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let storage = directory.path().join("storage");
    let original = paused_download(&harness, &storage, "one", SOURCE).await;
    let copy = paused_download(&harness, &storage, "two", "https://other.invalid/x.bin").await;
    let original_path = storage.join("one").join("file.bin");
    let copy_path = storage.join("two").join("file.bin");
    finished(&harness, &original, &original_path, b"identical bytes").await;
    finished(&harness, &copy, &copy_path, b"identical bytes").await;
    let duplicates = format!("/api/v1/downloads/{}/duplicates", copy.id);
    let (status, report) = common::get_json(&harness.router, &duplicates).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["content"].as_array().expect("content").len(), 1);
    assert_eq!(preview(&harness.router).await["content_index"], 2);

    let (status, body) = common::post_json(
        &harness.router,
        "/api/v1/storage/content-index/clear",
        json!({ "confirmed": true }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 2, "{body}");
    assert_eq!(preview(&harness.router).await["content_index"], 0);
    assert_eq!(
        audited_count(&harness.router, "content_index_cleared").await,
        "2"
    );
    for path in [&original_path, &copy_path] {
        assert!(path.is_file(), "a file went with the index: {path:?}");
    }
    let (status, report) = common::get_json(&harness.router, &duplicates).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(
        report["content"].as_array().expect("content").is_empty(),
        "an emptied index knows no content duplicate: {report}"
    );
    let (status, refused) = common::post_json(
        &harness.router,
        &format!("/api/v1/downloads/{}/dedupe", copy.id),
        json!({ "original_download_id": original.id, "mode": "hardlink" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "storage.dedupe_not_indexed");

    let (status, check) = common::post_json(
        &harness.router,
        "/api/v1/storage/content-index/check",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{check}");
    assert_eq!(check["backfilled"], 2, "{check}");
    let (status, report) = common::get_json(&harness.router, &duplicates).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(
        report["content"].as_array().expect("content").len(),
        1,
        "the check found both files again through their rows: {report}"
    );
}

/// Checking the index is queue work; throwing it or the history away is not.
#[tokio::test]
async fn a_queue_token_checks_the_index_but_clears_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(
        directory.path(),
        common::Options::default()
            .login()
            .parked()
            .token(QUEUE_BEARER, rd_core::API_QUEUE_SCOPE)
            .token(ADMIN_BEARER, rd_core::API_ADMIN_SCOPE),
    )
    .await;
    let confirmed = || json!({ "confirmed": true });

    let (status, body) = common::post_with_bearer(
        &harness.router,
        "/api/v1/storage/content-index/check",
        QUEUE_BEARER,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for path in [
        "/api/v1/storage/operations/clear",
        "/api/v1/storage/content-index/clear",
    ] {
        let (status, body) =
            common::post_with_bearer(&harness.router, path, QUEUE_BEARER, confirmed()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        let (status, body) =
            common::post_with_bearer(&harness.router, path, ADMIN_BEARER, confirmed()).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        assert_eq!(body["removed"], 0, "{path}: {body}");
    }
}
