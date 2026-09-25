//! Manual order inside one package — links in the LinkGrabber, files in the download queue.
//!
//! Both endpoints take the package's complete id list and hand out the positions 1..n from it.
//! What is checked here is the half that only shows end to end: that an incomplete or foreign
//! list is refused with the same stable code on both sides instead of being written, or — as
//! the candidate endpoint used to do — quietly ignored while answering "Order saved".

mod common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_harness};
use serde_json::json;

const MISMATCH: &str = "request.reorder_ids_mismatch";

/// One LinkGrabber package with three online-ready links.
async fn submit(harness: &common::Harness) {
    let names = ["a.bin", "b.bin", "c.bin"];
    harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Release".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            providers: names.iter().map(|_| None).collect(),
            file_names: names.iter().map(|name| Some((*name).to_owned())).collect(),
            sizes: names.iter().map(|_| None).collect(),
            requests: names.iter().map(|_| None).collect(),
            body_refs: names.iter().map(|_| None).collect(),
            urls: names
                .iter()
                .map(|name| {
                    format!("https://files.example.com/{name}")
                        .parse()
                        .expect("url")
                })
                .collect(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
}

fn ids(rows: &serde_json::Value) -> Vec<String> {
    rows.as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["id"].as_str().map(ToOwned::to_owned))
        .collect()
}

fn names(rows: &serde_json::Value) -> Vec<String> {
    rows.as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| {
            row["file_name"]
                .as_str()
                .map(ToOwned::to_owned)
                .or_else(|| row["name"].as_str().map(ToOwned::to_owned))
        })
        .collect()
}

/// Enqueues the single collector package and returns `(package id, file ids in queue order)`.
async fn enqueue(harness: &common::Harness) -> (String, Vec<String>) {
    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    let collector_ids = ids(&packages);
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": collector_ids }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (_, packages) = get_json(&harness.router, "/api/v1/packages").await;
    let package_id = ids(&packages).first().cloned().expect("queue package");
    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    (package_id, ids(&downloads))
}

#[tokio::test]
async fn download_order_is_written_only_for_a_complete_list_of_the_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(&harness).await;
    let (package_id, files) = enqueue(&harness).await;
    assert_eq!(files.len(), 3);

    // Two of three: the endpoint would otherwise renumber the pair and shove the third to the end.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reorder",
        json!({ "package_id": package_id, "ids": [files[0], files[1]] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], MISMATCH, "{body}");

    // The same id twice is a complete-looking list that names only two rows.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reorder",
        json!({ "package_id": package_id, "ids": [files[0], files[0], files[1]] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], MISMATCH, "{body}");

    // An id from nowhere near this package used to be a silent no-op.
    let foreign = rd_core::DownloadId::new().to_string();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reorder",
        json!({ "package_id": package_id, "ids": [files[0], files[1], foreign] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], MISMATCH, "{body}");

    let (_, unchanged) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(names(&unchanged), ["a.bin", "b.bin", "c.bin"]);

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/reorder",
        json!({ "package_id": package_id, "ids": [files[2], files[0], files[1]] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "download.order_saved", "{body}");
    let (_, reordered) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(names(&reordered), ["c.bin", "a.bin", "b.bin"]);
}

#[tokio::test]
async fn candidate_order_rejects_the_same_lists_as_the_download_order() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    submit(&harness).await;
    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    let package_id = ids(&packages).first().cloned().expect("collector package");
    let (_, candidates) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    let links = ids(&candidates);
    assert_eq!(links.len(), 3);

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/candidates/reorder",
        json!({ "package_id": package_id, "ids": [links[0], links[1]] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], MISMATCH, "{body}");

    let foreign = rd_core::CandidateId::new().to_string();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/candidates/reorder",
        json!({ "package_id": package_id, "ids": [links[0], links[1], foreign] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], MISMATCH, "{body}");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/candidates/reorder",
        json!({ "package_id": package_id, "ids": [links[1], links[2], links[0]] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, reordered) = get_json(&harness.router, "/api/v1/collector/candidates").await;
    assert_eq!(names(&reordered), ["b.bin", "c.bin", "a.bin"]);
}
