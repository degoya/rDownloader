//! RD-080-10: RSS, Atom and podcast subscriptions end to end.
//!
//! The parser is unit-tested in `rd-subscription`; what runs here is the whole path — a feed
//! is fetched, its items are archived exactly once, the backlog is not imported on
//! activation, and an unchanged feed costs a `304` rather than a re-import.

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use common::{get_json, post_json, test_router};
use serde_json::{Value, json};

const ETAG: &str = "\"feed-v1\"";

const FEED: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
  <channel>
    <title>The Show</title>
    <language>en</language>
    <item>
      <title>Episode One</title>
      <guid>tag:example,2026:1</guid>
      <pubDate>Wed, 04 Feb 2026 13:00:00 GMT</pubDate>
      <enclosure url="https://cdn.example.test/1.mp3" length="1234" type="audio/mpeg"/>
      <itunes:duration>1:02:03</itunes:duration>
    </item>
    <item>
      <title>Episode Two</title>
      <guid>tag:example,2026:2</guid>
      <pubDate>Thu, 05 Feb 2026 13:00:00 GMT</pubDate>
      <enclosure url="https://cdn.example.test/2.mp3" length="2345" type="audio/mpeg"/>
    </item>
  </channel>
</rss>"#;

/// Counts requests and honours `If-None-Match`, so caching can be asserted on.
#[derive(Clone, Default)]
struct FeedState {
    requests: Arc<AtomicUsize>,
    conditional: Arc<AtomicUsize>,
}

async fn serve_feed(State(state): State<FeedState>, headers: HeaderMap) -> impl IntoResponse {
    state.requests.fetch_add(1, Ordering::SeqCst);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(ETAG)
    {
        state.conditional.fetch_add(1, Ordering::SeqCst);
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, ETAG)],
            String::new(),
        );
    }
    (StatusCode::OK, [(header::ETAG, ETAG)], FEED.to_owned())
}

async fn feed_server() -> (String, FeedState) {
    let state = FeedState::default();
    let app = Router::new()
        .route("/feed.xml", get(serve_feed))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}/feed.xml"), state)
}

fn body(url: &str, backlog: Value) -> Value {
    json!({
        "name": "The Show",
        "url": url,
        "kind": "feed",
        "enabled": true,
        "mode": "review",
        "interval_seconds": 3_600,
        "filters": {},
        "backlog": backlog
    })
}

/// Creates a subscription, polls it, and waits for the run to be recorded.
async fn create_and_poll(router: &Router, url: &str, backlog: Value) -> String {
    let (status, created) = post_json(router, "/api/v1/subscriptions", body(url, backlog)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    poll(router, &id).await;
    id
}

async fn poll(router: &Router, id: &str) {
    let before = run_count(router, id).await;
    let (status, _) = post_json(
        router,
        &format!("/api/v1/subscriptions/{id}/poll"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    for _ in 0..200 {
        if run_count(router, id).await > before {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("poll never finished");
}

async fn run_count(router: &Router, id: &str) -> usize {
    let (_, runs) = get_json(router, &format!("/api/v1/subscriptions/{id}/runs")).await;
    runs.as_array().map(Vec::len).unwrap_or_default()
}

async fn items(router: &Router, id: &str) -> Vec<Value> {
    let (_, items) = get_json(router, &format!("/api/v1/subscriptions/{id}/items")).await;
    items.as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn activating_a_feed_does_not_import_its_backlog() {
    // The default policy. A podcast with 400 episodes must not enter the queue on the day
    // it is subscribed to.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = feed_server().await;
    let id = create_and_poll(&router, &url, json!({ "mode": "from_now" })).await;

    let items = items(&router, &id).await;
    assert_eq!(items.len(), 2, "both items should be archived");
    assert!(
        items.iter().all(|item| item["state"] == "skipped"),
        "existing items must not be acted on: {items:?}"
    );
    assert!(
        items.iter().all(|item| item["reason"] == "backlog"),
        "and the reason must say why: {items:?}"
    );
}

#[tokio::test]
async fn asking_to_review_the_backlog_collects_it_instead() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = feed_server().await;
    let id = create_and_poll(&router, &url, json!({ "mode": "review_all" })).await;

    let items = items(&router, &id).await;
    assert_eq!(items.len(), 2);
    assert!(
        items.iter().all(|item| item["state"] == "pending"),
        "the backlog should be waiting for review: {items:?}"
    );
}

#[tokio::test]
async fn polling_twice_archives_each_item_once() {
    // The guid is stable, so a second poll of an unchanged feed must add nothing — this is
    // the guarantee the whole feature rests on.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = feed_server().await;
    let id = create_and_poll(&router, &url, json!({ "mode": "review_all" })).await;
    assert_eq!(items(&router, &id).await.len(), 2);

    poll(&router, &id).await;
    assert_eq!(
        items(&router, &id).await.len(),
        2,
        "a repeated poll must not duplicate the archive"
    );

    let (_, runs) = get_json(&router, &format!("/api/v1/subscriptions/{id}/runs")).await;
    let runs = runs.as_array().expect("runs");
    assert_eq!(runs.len(), 2);
    // The second run found the same items and accepted none of them.
    assert_eq!(runs[0]["accepted"], 0);
}

#[tokio::test]
async fn an_unchanged_feed_is_fetched_conditionally() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, state) = feed_server().await;
    let id = create_and_poll(&router, &url, json!({ "mode": "from_now" })).await;
    poll(&router, &id).await;

    assert!(state.requests.load(Ordering::SeqCst) >= 2);
    assert!(
        state.conditional.load(Ordering::SeqCst) >= 1,
        "the stored ETag should have produced a 304 rather than a second download"
    );
}

#[tokio::test]
async fn podcast_metadata_reaches_the_archive() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (url, _) = feed_server().await;
    let id = create_and_poll(&router, &url, json!({ "mode": "review_all" })).await;

    let items = items(&router, &id).await;
    let episode = items
        .iter()
        .find(|item| item["title"] == "Episode One")
        .expect("episode one");
    // The enclosure, not the show-notes page.
    assert_eq!(episode["url"], "https://cdn.example.test/1.mp3");
    assert_eq!(episode["duration_seconds"], 3_723);
    assert!(episode["published_at"].is_string());
}

#[tokio::test]
async fn a_feed_that_cannot_be_reached_fails_its_own_subscription_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let (working, _) = feed_server().await;
    let good = create_and_poll(&router, &working, json!({ "mode": "review_all" })).await;
    // A port nothing is listening on.
    let broken = create_and_poll(
        &router,
        "http://127.0.0.1:1/feed.xml",
        json!({ "mode": "review_all" }),
    )
    .await;

    let (_, subscriptions) = get_json(&router, "/api/v1/subscriptions").await;
    let subscriptions = subscriptions.as_array().expect("array");
    let broken_row = subscriptions
        .iter()
        .find(|row| row["id"] == broken.as_str())
        .expect("broken row");
    assert!(broken_row["last_error"].is_string(), "{broken_row}");
    assert_eq!(broken_row["consecutive_failures"], 1);

    // The working one is untouched by its neighbour's failure.
    let good_row = subscriptions
        .iter()
        .find(|row| row["id"] == good.as_str())
        .expect("good row");
    assert!(good_row["last_error"].is_null(), "{good_row}");
    assert_eq!(items(&router, &good).await.len(), 2);
}
