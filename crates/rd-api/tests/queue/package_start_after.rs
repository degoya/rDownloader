//! A package's "not before" over REST (RD-1240-14): set, read back on the package, removed, a
//! moment that has passed stored as none, and an unknown package refused.

use crate::common;

use axum::http::StatusCode;
use chrono::{DateTime, Duration, SubsecRound, Utc};
use serde_json::json;

/// One package with one direct download; answers its id.
async fn package(harness: &common::Harness) -> String {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": "https://example.invalid/tonight.bin", "package_name": "Tonight" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["package_id"]
        .as_str()
        .expect("package id")
        .to_owned()
}

/// The `start_after` of the package as the package list shows it.
async fn listed(harness: &common::Harness, id: &str) -> serde_json::Value {
    let (status, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    assert_eq!(status, StatusCode::OK, "{packages}");
    packages
        .as_array()
        .expect("packages")
        .iter()
        .find(|package| package["id"] == id)
        .expect("listed")["start_after"]
        .clone()
}

#[tokio::test]
async fn the_moment_is_set_shown_on_the_package_and_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let id = package(&harness).await;
    let uri = format!("/api/v1/packages/{id}/start-after");

    let at = (Utc::now() + Duration::hours(5)).trunc_subsecs(0);
    let (status, stored) =
        common::put_json(&harness.router, &uri, json!({ "start_after": at })).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let answered: DateTime<Utc> =
        serde_json::from_value(stored["start_after"].clone()).expect("moment");
    assert_eq!(answered, at);
    let shown: DateTime<Utc> = serde_json::from_value(listed(&harness, &id).await).expect("shown");
    assert_eq!(shown, at);

    // A moment that has passed holds nothing, so it is stored as none.
    let past = Utc::now() - Duration::minutes(5);
    let (status, stored) =
        common::put_json(&harness.router, &uri, json!({ "start_after": past })).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(stored["start_after"], serde_json::Value::Null);
    assert_eq!(listed(&harness, &id).await, serde_json::Value::Null);

    let (status, stored) =
        common::put_json(&harness.router, &uri, json!({ "start_after": at })).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let (status, stored) = common::put_json(&harness.router, &uri, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(listed(&harness, &id).await, serde_json::Value::Null);
}

#[tokio::test]
async fn an_unknown_package_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (status, refused) = common::put_json(
        &harness.router,
        "/api/v1/packages/0190a1b2-0000-7000-8000-000000000001/start-after",
        json!({ "start_after": Utc::now() + Duration::hours(1) }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["code"], "package.not_found");
}
