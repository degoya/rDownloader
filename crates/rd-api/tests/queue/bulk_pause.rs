//! Pausing and resuming many files in one request and one writer transaction (RD-1250-08).

use crate::common;

use axum::http::StatusCode;
use common::{get_json, parked_harness, post_json};
use serde_json::json;

/// Every event that has arrived; panics when the subscriber fell behind, which one event per
/// row would have made it.
fn drained(
    events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>,
) -> Vec<rd_core::EventEnvelope> {
    let mut arrived = Vec::new();
    loop {
        match events.try_recv() {
            Ok(event) => arrived.push(event),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(missed)) => {
                panic!("the subscriber fell {missed} events behind")
            }
            Err(_) => return arrived,
        }
    }
}

/// The `download.state` events among `arrived` that moved rows to `state`.
fn moved_to<'a>(
    arrived: &'a [rd_core::EventEnvelope],
    state: &str,
) -> Vec<&'a rd_core::EventEnvelope> {
    arrived
        .iter()
        .filter(|event| {
            event.kind == rd_core::EventKind::DownloadState && event.payload["state"] == state
        })
        .collect()
}

/// Sends one bulk action by the list filter and answers its body and how long it took.
async fn by_filter(
    harness: &common::Harness,
    action: &str,
    states: serde_json::Value,
) -> (serde_json::Value, std::time::Duration) {
    let started = std::time::Instant::now();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        json!({ "action": action, "ids": [], "filter": { "states": states } }),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(status, StatusCode::OK, "{action}: {body}");
    (body, elapsed)
}

/// A thousand files resumed and paused again, each direction one request, one transaction and
/// one event. Before RD-1250-08 the web interface's "pause all" sent one request per file and
/// the server wrote each in a transaction of its own, with an event each and the whole package
/// read again after every file; the times are printed for the job file.
#[tokio::test]
async fn a_thousand_downloads_are_resumed_and_paused_in_one_request_each() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let rows =
        crate::download_list::package_of(&harness, directory.path(), "Bulk.Pause", 1000).await;
    let mut events = harness.database.subscribe();

    let (resumed, resumed_in) = by_filter(&harness, "resume", json!(["paused"])).await;
    let pausable = json!([
        "queued",
        "retry_wait",
        "resolving",
        "downloading",
        "verifying",
        "repairing",
        "extracting"
    ]);
    let (paused, paused_in) = by_filter(&harness, "pause", pausable).await;
    eprintln!("RD-1250-08: 1000 downloads resumed in {resumed_in:?}, paused in {paused_in:?}");
    for body in [&resumed, &paused] {
        assert_eq!(body["affected"], 1000, "{body}");
        assert!(
            body["refusals"].as_array().is_some_and(Vec::is_empty),
            "{body}"
        );
    }
    for elapsed in [resumed_in, paused_in] {
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "1000 files took {elapsed:?}, no faster than one transaction per file"
        );
    }

    // Drained once: each call empties the channel, so a second one would find the pause's
    // event already gone.
    let arrived = drained(&mut events);
    let queued = moved_to(&arrived, "queued");
    assert_eq!(queued.len(), 1, "one event for the resume");
    assert_eq!(queued[0].payload["count"], 1000);
    let paused = moved_to(&arrived, "paused");
    assert_eq!(paused.len(), 1, "one event for the pause");
    assert_eq!(paused[0].payload["count"], 1000);

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let listed = downloads.as_array().expect("rows");
    assert_eq!(listed.len(), rows.len());
    assert!(
        listed.iter().all(|row| row["state"] == "paused"),
        "every file paused again"
    );
}

/// By ids a refusal stays per id with the code the single route answers: an unknown id. The
/// rest of the batch goes ahead, and a second mention of a file queues it again, as a second
/// single resume does.
#[tokio::test]
async fn a_refused_id_leaves_the_rest_of_the_batch_resumed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let rows =
        crate::download_list::package_of(&harness, directory.path(), "Partly.Resumed", 3).await;
    let unknown = rd_core::DownloadId::new();

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        json!({ "action": "resume", "ids": [rows[0], unknown, rows[1], rows[0]] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["affected"], 3, "{body}");
    let codes: Vec<_> = body["refusals"]
        .as_array()
        .expect("refusals")
        .iter()
        .map(|refusal| refusal["code"].clone())
        .collect();
    assert_eq!(codes, [json!("download.not_found")], "{body}");

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let state = |id: rd_core::DownloadId| {
        downloads
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == id.to_string()))
            .map(|row| row["state"].clone())
    };
    assert_eq!(state(rows[0]), Some(json!("queued")));
    assert_eq!(state(rows[1]), Some(json!("queued")));
    assert_eq!(
        state(rows[2]),
        Some(json!("paused")),
        "not named, not moved"
    );
}
