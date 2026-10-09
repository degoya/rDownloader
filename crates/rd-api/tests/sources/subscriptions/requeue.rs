//! RD-1150-04: a subscription item is queued again, whatever was decided about it before.
//!
//! The way back is the first queueing's way: the subscription's category, the LinkGrabber, the
//! item `queued` and an audit record. What it must not do is double a link silently or hand over
//! an item that has nothing to fetch.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{body, common, get_json, post_json, script_category};

/// An indexer-like item of `state` at `url`.
fn item(key: &str, url: &str, state: rd_core::SubscriptionItemState) -> rd_db::NewSubscriptionItem {
    rd_db::NewSubscriptionItem {
        item_key: key.to_owned(),
        title: format!("Some.Release.{key}"),
        url: url.parse().expect("url"),
        published_at: None,
        duration_seconds: None,
        state,
        reason: None,
        source_category: None,
        media_type: None,
        attributes: std::collections::BTreeMap::new(),
        password: None,
    }
}

/// A subscription filing into its own category, with `items` stored as a poll would store them.
async fn seeded(
    harness: &common::Harness,
    directory: &std::path::Path,
    items: Vec<rd_db::NewSubscriptionItem>,
) -> (String, String) {
    let category = script_category(&harness.router, directory).await;
    let mut request = body("Requeue source");
    request["category_id"] = json!(category);
    let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    harness
        .database
        .record_subscription_items(id.parse().expect("subscription id"), items)
        .await
        .expect("items");
    (id, category)
}

/// The id of the stored item `key`, and its state.
async fn stored(router: &axum::Router, id: &str, key: &str) -> (String, String) {
    let (_, page) = get_json(
        router,
        &format!("/api/v1/subscriptions/{id}/items/page?state=all&limit=200"),
    )
    .await;
    let row = page["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|row| row["item_key"] == key)
        .unwrap_or_else(|| panic!("{key} is not stored: {page}"))
        .clone();
    (
        row["id"].as_str().expect("item id").to_owned(),
        row["state"].as_str().expect("state").to_owned(),
    )
}

async fn requeue(router: &axum::Router, id: &str, request: Value) -> Value {
    let (status, answer) = post_json(
        router,
        &format!("/api/v1/subscriptions/{id}/items/requeue"),
        request,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer
}

/// The LinkGrabber's candidates at `url`, with their states and package categories.
async fn candidates_at(harness: &common::Harness, url: &str) -> Vec<(String, Option<String>)> {
    let packages = harness
        .database
        .list_collector_packages()
        .await
        .expect("packages");
    harness
        .database
        .list_candidates()
        .await
        .expect("candidates")
        .into_iter()
        .filter(|candidate| candidate.url.as_str() == url)
        .map(|candidate| {
            let category = packages
                .iter()
                .find(|package| Some(package.id) == candidate.package_id)
                .and_then(|package| package.category_id)
                .map(|category| category.to_string());
            (
                serde_json::to_value(candidate.state)
                    .expect("state")
                    .as_str()
                    .expect("state word")
                    .to_owned(),
                category,
            )
        })
        .collect()
}

#[tokio::test]
async fn a_dismissed_item_goes_back_to_the_linkgrabber_in_its_category_and_is_audited() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let url = "https://requeue.test/dismissed.nzb";
    let (id, category) = seeded(
        &harness,
        directory.path(),
        vec![item(
            "dismissed",
            url,
            rd_core::SubscriptionItemState::Dismissed,
        )],
    )
    .await;
    let (item_id, _) = stored(&harness.router, &id, "dismissed").await;

    let answer = requeue(&harness.router, &id, json!({ "item_ids": [item_id] })).await;
    assert_eq!(answer, json!({ "requeued": [item_id], "refused": [] }));
    assert_eq!(stored(&harness.router, &id, "dismissed").await.1, "queued");
    let candidates = candidates_at(&harness, url).await;
    assert_eq!(candidates.len(), 1, "{candidates:?}");
    assert_eq!(candidates[0].1.as_deref(), Some(category.as_str()));

    let (status, records) = get_json(
        &harness.router,
        "/api/v1/audit/records?action=subscription_item_requeued&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{records}");
    let rows = records["records"].as_array().expect("records");
    assert_eq!(rows.len(), 1, "{records}");
    assert_eq!(rows[0]["target_id"], item_id.as_str());
    assert_eq!(rows[0]["details"]["previous_state"], "dismissed");
    assert_eq!(rows[0]["details"]["duplicate"], "false");
}

/// A queued item whose download is gone -- finished and cleared, or deleted -- has nothing left
/// in the LinkGrabber or the list, so it is queued again like a new one.
#[tokio::test]
async fn a_queued_item_whose_download_is_gone_is_queued_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let url = "https://requeue.test/finished.nzb";
    let (id, _) = seeded(
        &harness,
        directory.path(),
        vec![item(
            "finished",
            url,
            rd_core::SubscriptionItemState::Queued,
        )],
    )
    .await;
    let (item_id, _) = stored(&harness.router, &id, "finished").await;

    let answer = requeue(&harness.router, &id, json!({ "item_ids": [item_id] })).await;
    assert_eq!(answer["requeued"], json!([item_id]), "{answer}");
    assert_eq!(candidates_at(&harness, url).await.len(), 1);
}

/// The second time the address is still in the LinkGrabber: refused with a code, not doubled --
/// and queued anyway when the caller says so, marked as the duplicate it is.
#[tokio::test]
async fn an_address_still_in_the_linkgrabber_is_refused_unless_duplicates_are_allowed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let url = "https://requeue.test/twice.nzb";
    let (id, _) = seeded(
        &harness,
        directory.path(),
        vec![item("twice", url, rd_core::SubscriptionItemState::Skipped)],
    )
    .await;
    let (item_id, _) = stored(&harness.router, &id, "twice").await;
    requeue(&harness.router, &id, json!({ "item_ids": [item_id] })).await;

    let answer = requeue(&harness.router, &id, json!({ "item_ids": [item_id] })).await;
    assert_eq!(answer["requeued"], json!([]), "{answer}");
    assert_eq!(answer["refused"][0]["item_id"], item_id.as_str());
    assert_eq!(answer["refused"][0]["code"], "subscription.item_duplicate");
    assert_eq!(candidates_at(&harness, url).await.len(), 1);

    let answer = requeue(
        &harness.router,
        &id,
        json!({ "item_ids": [item_id], "allow_duplicate": true }),
    )
    .await;
    assert_eq!(answer["requeued"], json!([item_id]), "{answer}");
    // The duplicate mark comes with the online check; read the states once it has settled.
    common::wait_for_candidates_ready(&harness.router).await;
    let states: Vec<String> = candidates_at(&harness, url)
        .await
        .into_iter()
        .map(|(state, _)| state)
        .collect();
    assert_eq!(states.len(), 2, "{states:?}");
    assert!(
        states.iter().any(|state| state == "duplicate"),
        "{states:?}"
    );
}

/// Nothing to fetch, or not this subscription's: refused per item with a code, the state kept,
/// and the rest of the request still done.
#[tokio::test]
async fn an_item_without_a_source_or_of_another_subscription_is_refused_with_a_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let (id, _) = seeded(
        &harness,
        directory.path(),
        vec![
            item(
                "nothing",
                "urn:release:nothing",
                rd_core::SubscriptionItemState::Dismissed,
            ),
            item(
                "fine",
                "https://requeue.test/fine.nzb",
                rd_core::SubscriptionItemState::Dismissed,
            ),
        ],
    )
    .await;
    let (nothing, _) = stored(&harness.router, &id, "nothing").await;
    let (fine, _) = stored(&harness.router, &id, "fine").await;
    let (status, other) = post_json(&harness.router, "/api/v1/subscriptions", body("Other")).await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    let other = other["id"].as_str().expect("id");

    let answer = requeue(&harness.router, &id, json!({ "item_ids": [nothing, fine] })).await;
    assert_eq!(answer["requeued"], json!([fine]), "{answer}");
    assert_eq!(answer["refused"][0]["code"], "subscription.item_no_source");
    assert_eq!(stored(&harness.router, &id, "nothing").await.1, "dismissed");

    let answer = requeue(&harness.router, other, json!({ "item_ids": [fine] })).await;
    assert_eq!(answer["refused"][0]["code"], "subscription.item_not_found");

    let (status, refused) = post_json(
        &harness.router,
        &format!("/api/v1/subscriptions/{id}/items/requeue"),
        json!({ "item_ids": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "request.bulk_range");
}
