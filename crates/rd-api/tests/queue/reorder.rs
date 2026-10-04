//! Manual order inside one package — links in the LinkGrabber, files in the download queue.
//!
//! Both endpoints take the package's complete id list and hand out the positions 1..n from it.
//! What is checked here is the half that only shows end to end: that an incomplete or foreign
//! list is refused with the same stable code on both sides instead of being written, or — as
//! the candidate endpoint used to do — quietly ignored while answering "Order saved".

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_harness};
use serde_json::json;

const MISMATCH: &str = "request.reorder_ids_mismatch";

/// One LinkGrabber package with three online-ready links.
async fn submit(harness: &common::Harness) {
    submit_release(harness, "Release").await;
}

/// [`submit`] under another package name, with addresses of its own, so several releases can
/// stand side by side without one being flagged as the other's duplicate.
async fn submit_release(harness: &common::Harness, release: &str) {
    let names = ["a.bin", "b.bin", "c.bin"];
    harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some(release.to_owned()),
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
                    format!("https://files.example.com/{release}/{name}")
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

/// A list route pages its own order: no parameters is the whole list without the count
/// header, a window is a slice of the same order with the count, an offset past the end is an
/// empty page, and a page size outside 1..=1000 or a value that is no number is refused (API-15).
async fn pages_its_own_order(harness: &common::Harness, path: &str, rows: usize) {
    let (whole, total) = page(harness, path).await;
    let whole = ids(&whole);
    assert_eq!(whole.len(), rows, "{path}");
    assert_eq!(total, None, "{path} answers unpaged without the count");

    let (window, total) = page(harness, &format!("{path}?limit=2&offset=1")).await;
    assert_eq!(ids(&window), whole[1..3], "{path}");
    assert_eq!(total.as_deref(), Some(rows.to_string().as_str()), "{path}");

    let (first, _) = page(harness, &format!("{path}?limit=1")).await;
    assert_eq!(ids(&first), whole[..1], "{path}");
    let (rest, _) = page(harness, &format!("{path}?offset=1")).await;
    assert_eq!(ids(&rest), whole[1..], "{path}");
    let (beyond, total) = page(harness, &format!("{path}?offset=99")).await;
    assert!(ids(&beyond).is_empty(), "{path}");
    assert_eq!(total.as_deref(), Some(rows.to_string().as_str()), "{path}");

    // A value that is no number at all is refused with the same code, not axum's uncoded text.
    for query in [
        "limit=0",
        "limit=1001",
        "limit=abc",
        "limit=-1",
        "limit=",
        "offset=abc",
    ] {
        let (status, body) = get_json(&harness.router, &format!("{path}?{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
        assert_eq!(body["code"], "request.page_limit", "{path}: {body}");
        assert_eq!(body["params"]["max"], "1000", "{path}: {body}");
    }
}

#[tokio::test]
async fn the_growing_lists_page_without_changing_their_unpaged_answer() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    for release in ["One", "Two", "Three"] {
        submit_release(&harness, release).await;
    }
    pages_its_own_order(&harness, "/api/v1/collector/candidates", 9).await;
    pages_its_own_order(&harness, "/api/v1/collector/packages", 3).await;
    pages_its_own_order(&harness, "/api/v1/collector/batches", 3).await;

    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": ids(&packages) }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    pages_its_own_order(&harness, "/api/v1/downloads", 9).await;
    pages_its_own_order(&harness, "/api/v1/packages", 3).await;
}
