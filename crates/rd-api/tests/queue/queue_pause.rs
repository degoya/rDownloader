//! Pausing the whole queue for a while over REST (RD-190-20): the end, what it stops, ending it
//! early and the refused ends. The restart cases are the scheduler's own
//! (`crates/rd-scheduler/tests/queue_pause.rs`).

use crate::common;

use axum::http::StatusCode;

async fn queued_download(router: &axum::Router) -> String {
    let (status, created) = common::post_json(
        router,
        "/api/v1/downloads",
        serde_json::json!({
            "url": "https://example.invalid/movie.mkv",
            "package_name": "Example Package"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("download id").to_owned()
}

async fn state_of(router: &axum::Router, id: &str) -> String {
    let (status, downloads) = common::get_json(router, "/api/v1/downloads").await;
    assert_eq!(status, StatusCode::OK, "{downloads}");
    downloads
        .as_array()
        .expect("downloads")
        .iter()
        .find(|download| download["id"] == id)
        .and_then(|download| download["state"].as_str())
        .expect("state")
        .to_owned()
}

#[tokio::test]
async fn a_timed_pause_stops_the_queue_until_it_is_ended() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let id = queued_download(router).await;

    let (status, idle) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(status, StatusCode::OK, "{idle}");
    assert_eq!(idle["paused"], false);
    assert!(idle["until"].is_null(), "{idle}");

    let before = chrono::Utc::now();
    let (status, paused) = common::put_json(
        router,
        "/api/v1/queue/pause",
        serde_json::json!({ "minutes": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    assert_eq!(paused["paused"], true);
    assert_eq!(paused["files"], 1);
    let until: chrono::DateTime<chrono::Utc> = paused["until"]
        .as_str()
        .expect("until")
        .parse()
        .expect("a time");
    assert!(until >= before + chrono::Duration::minutes(30), "{paused}");
    assert!(
        until <= chrono::Utc::now() + chrono::Duration::minutes(30),
        "{paused}"
    );
    assert_eq!(state_of(router, &id).await, "paused");

    let (_, read) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(read["paused"], true, "{read}");
    assert_eq!(read["until"], paused["until"]);

    let (status, ended) = common::delete_json(router, "/api/v1/queue/pause").await;
    assert_eq!(status, StatusCode::OK, "{ended}");
    assert_eq!(ended["resumed"], 1);
    assert_eq!(state_of(router, &id).await, "queued");
    let (_, read) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(read["paused"], false, "{read}");
}

#[tokio::test]
async fn an_end_time_is_kept_as_given() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let until = (chrono::Utc::now() + chrono::Duration::hours(3))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let (status, paused) = common::put_json(
        &harness.router,
        "/api/v1/queue/pause",
        serde_json::json!({ "until": until }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{paused}");
    let answered: chrono::DateTime<chrono::Utc> = paused["until"]
        .as_str()
        .expect("until")
        .parse()
        .expect("time");
    let asked: chrono::DateTime<chrono::Utc> = until.parse().expect("time");
    assert_eq!(answered, asked);
    assert_eq!(
        paused["files"], 0,
        "an empty queue pauses nothing, yet holds"
    );
}

#[tokio::test]
async fn an_end_that_is_missing_past_or_too_far_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let past = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    for body in [
        serde_json::json!({}),
        serde_json::json!({ "minutes": 0 }),
        serde_json::json!({ "minutes": 43_201 }),
        serde_json::json!({ "until": past }),
        serde_json::json!({ "minutes": 30, "until": past }),
    ] {
        let (status, refused) =
            common::put_json(&harness.router, "/api/v1/queue/pause", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {refused}");
        assert_eq!(
            refused["code"], "queue.pause_end_invalid",
            "{body}: {refused}"
        );
    }
    let (_, read) = common::get_json(&harness.router, "/api/v1/queue/pause").await;
    assert_eq!(
        read["paused"], false,
        "a refused pause must not be in force: {read}"
    );
}
