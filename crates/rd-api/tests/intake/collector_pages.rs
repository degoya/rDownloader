//! The LinkGrabber's three paged lists — batches, packages and links — cut their page in SQL
//! (RD-191-05): walked to the end the pages are the unpaged list in its own order, every page
//! carries the whole list's `X-Total-Count`, and a page past the end is empty.

use crate::common;

use axum::http::StatusCode;
use common::test_harness;

/// The ids of a list answer, in order, and its `X-Total-Count` when it carries one.
async fn list_page(harness: &common::Harness, uri: &str) -> (Vec<String>, Option<String>) {
    let (status, headers, bytes) = common::send_raw(
        &harness.router,
        common::request_to("GET", uri)
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    let rows: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let ids = rows
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["id"].as_str().map(ToOwned::to_owned))
        .collect();
    let total = headers
        .get("x-total-count")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    (ids, total)
}

/// Walks `list` in pages of two to one page past its end and holds every page to the unpaged
/// answer; `length` is how long the fixture made it.
async fn assert_pages_add_up(harness: &common::Harness, list: &str, length: usize) {
    let (whole, total) = list_page(harness, list).await;
    assert_eq!(whole.len(), length, "{list}");
    assert_eq!(total, None, "{list}: the unpaged list carries no count");
    let expected_total = length.to_string();

    let mut walked = Vec::new();
    for offset in (0..=length).step_by(2) {
        let (rows, total) = list_page(harness, &format!("{list}?limit=2&offset={offset}")).await;
        assert_eq!(
            total.as_deref(),
            Some(expected_total.as_str()),
            "{list} offset {offset}"
        );
        assert!(rows.len() <= 2, "{list} offset {offset}");
        walked.extend(rows);
    }
    assert_eq!(
        walked, whole,
        "{list}: the pages are the list, in its order"
    );

    let (beyond, total) = list_page(harness, &format!("{list}?limit=2&offset={length}")).await;
    assert!(beyond.is_empty(), "{list}: a page past the end is empty");
    assert_eq!(total.as_deref(), Some(expected_total.as_str()), "{list}");
    let (rest, _) = list_page(harness, &format!("{list}?offset=1")).await;
    assert_eq!(rest, whole[1..], "{list}: an offset alone is the rest");
}

#[tokio::test]
async fn the_linkgrabber_lists_are_paged_by_the_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    // Three batches of one package and two links each.
    for index in 0..3 {
        let urls: Vec<url::Url> = [
            format!("https://one.example/collector-pages-{index}-a.bin"),
            format!("https://two.example/collector-pages-{index}-b.bin"),
        ]
        .iter()
        .map(|url| url.parse().expect("url"))
        .collect();
        harness
            .database
            .add_collector_batch(rd_db::NewCollectorBatch {
                package_hints: Vec::new(),
                mirror_hints: Vec::new(),
                source: rd_core::IngressSource::Manual,
                source_label: None,
                package_name: Some(format!("Collector pages {index}")),
                password: None,
                passwords: Vec::new(),
                category_id: None,
                priority: None,
                providers: vec![None; urls.len()],
                file_names: Vec::new(),
                sizes: Vec::new(),
                requests: Vec::new(),
                body_refs: Vec::new(),
                urls,
                auto_check: false,
                source_attributes: Vec::new(),
            })
            .await
            .expect("batch");
    }

    assert_pages_add_up(&harness, "/api/v1/collector/batches", 3).await;
    assert_pages_add_up(&harness, "/api/v1/collector/packages", 3).await;
    assert_pages_add_up(&harness, "/api/v1/collector/candidates", 6).await;
}
