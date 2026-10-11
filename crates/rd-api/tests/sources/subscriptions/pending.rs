//! The bulk decision about a subscription's pending items.
//!
//! RD-1250-13: "all" in the review drawer is every page of the list as it was last drawn, and no
//! hit a check wrote after that: the drawer sends the list's watermark as `until`. A selection
//! names its items, and one of another subscription is not decided along with them.

use axum::http::StatusCode;
use serde_json::json;

use super::{body, common, create, get_json, post_json, put_json, test_harness, test_router};

/// A pending indexer-like item `key`.
fn pending(key: &str) -> rd_db::NewSubscriptionItem {
    rd_db::NewSubscriptionItem {
        item_key: key.to_owned(),
        title: format!("Release {key}"),
        url: format!("https://indexer.test/get/{key}.nzb")
            .parse()
            .expect("url"),
        published_at: None,
        duration_seconds: None,
        state: rd_core::SubscriptionItemState::Pending,
        reason: None,
        source_category: None,
        media_type: Some("application/x-nzb".to_owned()),
        attributes: std::collections::BTreeMap::new(),
        password: None,
    }
}

/// A subscription named `name` with the pending items `keys`, and its id.
///
/// An indexer subscription, because only those are in the review counts the drawer reads.
async fn seeded(harness: &common::Harness, name: &str, keys: &[&str]) -> String {
    let mut request = body(name);
    request["kind"] = json!("indexer");
    request["api_key"] = json!("super-secret-key");
    let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    harness
        .database
        .record_subscription_items(
            id.parse().expect("subscription id"),
            keys.iter().map(|key| pending(key)).collect(),
        )
        .await
        .expect("items");
    id
}

/// The id of the stored item `key` of subscription `id`.
async fn item_id(router: &axum::Router, id: &str, key: &str) -> String {
    let (_, page) = get_json(
        router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=all&limit=200"),
    )
    .await;
    page["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|row| row["item_key"] == key)
        .and_then(|row| row["id"].as_str())
        .expect("item id")
        .to_owned()
}

#[tokio::test]
async fn bulk_review_rejects_a_state_that_is_not_a_decision() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let created = create(&router, "Channel").await;
    let id = created["id"].as_str().expect("id");

    let (status, response) = put_json(
        &router,
        &format!("/api/v1/subscriptions/{id}/items/pending"),
        json!({ "state": "skipped" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "subscription.bulk_state_invalid");
}

#[tokio::test]
async fn bulk_review_with_item_ids_decides_only_the_named_pending_items() {
    let temp = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(temp.path()).await;
    let id = seeded(&harness, "Reviewed", &["seen-1", "seen-2", "arrived-later"]).await;
    let other = seeded(&harness, "Another search", &["foreign"]).await;
    let seen_1 = item_id(&harness.router, &id, "seen-1").await;
    let seen_2 = item_id(&harness.router, &id, "seen-2").await;
    let foreign = item_id(&harness.router, &other, "foreign").await;

    // The two on screen, plus one of another subscription, which is not this request's to decide.
    let (status, bulk) = put_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/pending"),
        json!({ "state": "dismissed", "item_ids": [seen_1, seen_2, foreign] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bulk}");
    assert_eq!(bulk, json!({ "matched": 2, "updated": 2, "failed": 0 }));

    let (_, left) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=pending"),
    )
    .await;
    assert_eq!(left["total"], 1, "{left}");
    assert_eq!(left["items"][0]["item_key"], "arrived-later");
    let (_, untouched) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{other}/items/page?state=pending"),
    )
    .await;
    assert_eq!(untouched["total"], 1, "{untouched}");

    // A decided item named again is no longer pending, so nothing matches.
    let (status, again) = put_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/pending"),
        json!({ "state": "queued", "item_ids": [seen_1] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again, json!({ "matched": 0, "updated": 0, "failed": 0 }));
}

#[tokio::test]
async fn bulk_review_until_the_watermark_decides_every_page_but_no_later_hit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(temp.path()).await;
    let keys: Vec<String> = (0..60).map(|number| format!("seen-{number:02}")).collect();
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    let id = seeded(&harness, "Reviewed", &keys).await;

    // The list as the drawer drew it: page one of two, and the watermark of both.
    let (_, page) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=pending&limit=50"),
    )
    .await;
    assert_eq!(page["items"].as_array().map(Vec::len), Some(50), "{page}");
    let watermark = page["newest_discovered_at"].clone();
    assert!(watermark.is_string(), "{page}");
    let (_, summary) = get_json(&harness.router, "/api/v1/subscriptions/review-summary").await;
    assert_eq!(
        summary["subscriptions"][0]["newest_discovered_at"],
        watermark
    );

    // A check writes a hit after the list was drawn.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    harness
        .database
        .record_subscription_items(id.parse().expect("subscription id"), vec![pending("later")])
        .await
        .expect("later item");

    let (status, bulk) = put_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/pending"),
        json!({ "state": "dismissed", "until": watermark }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bulk}");
    // Both pages, the ten on page two included.
    assert_eq!(bulk, json!({ "matched": 60, "updated": 60, "failed": 0 }));
    let (_, left) = get_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=pending"),
    )
    .await;
    assert_eq!(left["total"], 1, "{left}");
    assert_eq!(left["items"][0]["item_key"], "later");
    assert_ne!(left["newest_discovered_at"], watermark);
}
