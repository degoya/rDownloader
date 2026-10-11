//! A package's and a category's download window over REST (RD-1240-30): set, read back on the
//! package and on its own route, removed, refused when malformed, and an unknown id refused.

use crate::common;

use axum::http::StatusCode;
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

fn nightly(ignore_schedule_pause: bool) -> serde_json::Value {
    json!({
        "windows": [{ "days": 127, "start_minute": 1320, "end_minute": 360 }],
        "ignore_schedule_pause": ignore_schedule_pause,
    })
}

#[tokio::test]
async fn a_package_window_is_set_shown_and_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let id = package(&harness).await;
    let uri = format!("/api/v1/packages/{id}/download-window");

    let (status, stored) = common::put_json(
        &harness.router,
        &uri,
        json!({ "download_window": nightly(true) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(stored["download_window"], nightly(true));

    let (status, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    assert_eq!(status, StatusCode::OK, "{packages}");
    let listed = packages
        .as_array()
        .expect("packages")
        .iter()
        .find(|package| package["id"] == id.as_str())
        .expect("listed")["download_window"]
        .clone();
    assert_eq!(listed, nightly(true));

    let (status, read) = common::get_json(&harness.router, &uri).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["download_window"], nightly(true));
    assert_eq!(read["category_window"], serde_json::Value::Null);
    // With no profile in force nothing pauses downloads, so only the window can hold it.
    assert!(read["held"].is_null() || read["held"] == "window", "{read}");

    let (status, stored) = common::put_json(&harness.router, &uri, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(stored["download_window"], serde_json::Value::Null);
    let (_, read) = common::get_json(&harness.router, &uri).await;
    assert_eq!(read["held"], serde_json::Value::Null, "{read}");
}

#[tokio::test]
async fn a_malformed_window_and_an_unknown_package_are_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let id = package(&harness).await;
    let uri = format!("/api/v1/packages/{id}/download-window");
    for window in [
        json!({ "windows": [{ "days": 0, "start_minute": 0, "end_minute": 60 }] }),
        json!({ "windows": [{ "days": 1, "start_minute": 60, "end_minute": 60 }] }),
        json!({ "windows": [{ "days": 1, "start_minute": 1440, "end_minute": 60 }] }),
    ] {
        let (status, refused) =
            common::put_json(&harness.router, &uri, json!({ "download_window": window })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["code"], "download_window.window_invalid");
    }
    let unknown = "/api/v1/packages/0190a1b2-0000-7000-8000-000000000001/download-window";
    let (status, refused) = common::put_json(
        &harness.router,
        unknown,
        json!({ "download_window": nightly(false) }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["code"], "package.not_found");
    let (status, refused) = common::get_json(&harness.router, unknown).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
}

#[tokio::test]
async fn an_unknown_category_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (status, refused) = common::put_json(
        &harness.router,
        "/api/v1/categories/0190a1b2-0000-7000-8000-000000000002/download-window",
        json!({ "download_window": nightly(false) }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");
    assert_eq!(refused["code"], "category.not_found");
}
