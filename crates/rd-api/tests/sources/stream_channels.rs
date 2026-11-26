//! Changing and deleting a stream channel, and recording an address at once (audit 2026-11-25,
//! TST-7). The schedules on a channel have their suite (`stream_schedules`).

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, get_json, parked_harness, post_json, put_json, test_router};
use serde_json::{Value, json};

fn channel(name: &str) -> Value {
    json!({
        "url": "https://twitch.tv/example",
        "name": name,
        "quality": null,
        "category_id": null,
        "enabled": true,
    })
}

#[tokio::test]
async fn a_channel_is_changed_and_deleted_by_its_id() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, created) = post_json(&router, "/api/v1/streams/channels", channel("First")).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let uri = format!(
        "/api/v1/streams/channels/{}",
        created["id"].as_str().expect("an id")
    );

    let mut changed = channel("Renamed");
    changed["quality"] = json!("720p");
    changed["enabled"] = json!(false);
    let (status, body) = put_json(&router, &uri, changed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Renamed", "{body}");
    assert_eq!(body["quality"], "720p", "{body}");
    assert_eq!(body["enabled"], false, "{body}");

    let mut elsewhere = channel("Renamed");
    elsewhere["url"] = json!("file:///etc/passwd");
    let (status, body) = put_json(&router, &uri, elsewhere).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "stream.url_invalid", "{body}");
    let (_, channels) = get_json(&router, "/api/v1/streams/channels").await;
    assert_eq!(
        channels[0]["url"], "https://twitch.tv/example",
        "{channels}"
    );

    let (status, body) = delete_json(&router, &uri).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (_, channels) = get_json(&router, "/api/v1/streams/channels").await;
    assert_eq!(channels, json!([]));

    for (status, body) in [
        delete_json(&router, &uri).await,
        put_json(&router, &uri, channel("Ghost")).await,
    ] {
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["code"], "stream.channel_not_found", "{body}");
    }
}

#[tokio::test]
async fn recording_at_once_puts_a_recording_into_the_queue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = parked_harness(directory.path()).await.router;

    let (status, body) = post_json(
        &router,
        "/api/v1/streams/record",
        json!({ "url": "ftp://twitch.tv/example", "name": null, "quality": null, "category_id": null }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "stream.url_invalid", "{body}");

    let (status, package) = post_json(
        &router,
        "/api/v1/streams/record",
        json!({
            "url": "https://twitch.tv/example",
            "name": "Show",
            "quality": "best",
            "category_id": null,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{package}");
    let (_, downloads) = get_json(&router, "/api/v1/downloads").await;
    let rows = downloads.as_array().expect("a list");
    assert_eq!(rows.len(), 1, "{downloads}");
    assert_eq!(rows[0]["package_id"], package["id"], "{downloads}");
    assert_eq!(rows[0]["kind"], "record", "{downloads}");
    assert!(
        rows[0]["file_name"]
            .as_str()
            .is_some_and(|name| name.starts_with("Show-") && name.ends_with(".ts")),
        "{downloads}"
    );
}
