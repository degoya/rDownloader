//! The queue's stop mark over REST (RD-1210-02): set on a file or a package, named when read,
//! read with the pause, refused for a finished or a missing target, cleared, and acted on once
//! its file is done. The scheduler's own cases — reorder, delete, restart, the crash — are in
//! `crates/rd-scheduler/tests/stop_mark.rs`.

use crate::common;

use axum::http::StatusCode;

/// One queued download in a package of its own; answers its id and its package's id.
async fn queued_download(router: &axum::Router, name: &str) -> (String, String) {
    let (status, created) = common::post_json(
        router,
        "/api/v1/downloads",
        serde_json::json!({
            "url": format!("https://example.invalid/{name}"),
            "package_name": format!("Stop mark {name}")
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    (
        created["id"].as_str().expect("download id").to_owned(),
        created["package_id"]
            .as_str()
            .expect("package id")
            .to_owned(),
    )
}

#[tokio::test]
async fn a_mark_is_set_named_read_with_the_pause_and_cleared() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let (id, package_id) = queued_download(router, "stop-mark-first.bin").await;

    let (status, idle) = common::get_json(router, "/api/v1/queue/stop-mark").await;
    assert_eq!(status, StatusCode::OK, "{idle}");
    assert!(idle["stop_mark"].is_null(), "{idle}");

    let (status, set) = common::put_json(
        router,
        "/api/v1/queue/stop-mark",
        serde_json::json!({ "download_id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set["download_id"], id);
    assert!(set["package_id"].is_null(), "{set}");
    assert_eq!(set["name"], "stop-mark-first.bin");

    let (_, pause) = common::get_json(router, "/api/v1/queue/pause").await;
    assert_eq!(pause["stop_mark"]["download_id"], id, "{pause}");
    assert_eq!(
        pause["paused"], false,
        "a mark alone pauses nothing: {pause}"
    );

    // A new mark replaces the old one; on a package it carries the package's name.
    let (status, replaced) = common::put_json(
        router,
        "/api/v1/queue/stop-mark",
        serde_json::json!({ "package_id": package_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!(replaced["package_id"], package_id);
    assert_eq!(replaced["name"], "Stop mark stop-mark-first.bin");
    let (_, read) = common::get_json(router, "/api/v1/queue/stop-mark").await;
    assert_eq!(read["stop_mark"]["package_id"], package_id, "{read}");

    let (status, cleared) = common::delete_json(router, "/api/v1/queue/stop-mark").await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["cleared"], true);
    let (_, again) = common::delete_json(router, "/api/v1/queue/stop-mark").await;
    assert_eq!(again["cleared"], false, "{again}");
    let (_, read) = common::get_json(router, "/api/v1/queue/stop-mark").await;
    assert!(read["stop_mark"].is_null(), "{read}");
}

#[tokio::test]
async fn a_target_that_is_missing_finished_or_not_one_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let (id, package_id) = queued_download(router, "stop-mark-refused.bin").await;
    let nobody = "0192f0c4-0000-7000-8000-00000000abcd";

    for body in [
        serde_json::json!({}),
        serde_json::json!({ "download_id": id, "package_id": package_id }),
    ] {
        let (status, refused) =
            common::put_json(router, "/api/v1/queue/stop-mark", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {refused}");
        assert_eq!(refused["code"], "queue.stop_mark_target_invalid");
    }
    for (body, code) in [
        (
            serde_json::json!({ "download_id": nobody }),
            "download.not_found",
        ),
        (
            serde_json::json!({ "package_id": nobody }),
            "package.not_found",
        ),
    ] {
        let (status, refused) =
            common::put_json(router, "/api/v1/queue/stop-mark", body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}: {refused}");
        assert_eq!(refused["code"], code);
    }

    let (status, cancelled) = common::post_json(
        router,
        &format!("/api/v1/downloads/{id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert!(status.is_success(), "{cancelled}");
    for body in [
        serde_json::json!({ "download_id": id }),
        serde_json::json!({ "package_id": package_id }),
    ] {
        let (status, refused) =
            common::put_json(router, "/api/v1/queue/stop-mark", body.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}: {refused}");
        assert_eq!(refused["code"], "queue.stop_mark_target_finished");
    }
    let (_, read) = common::get_json(router, "/api/v1/queue/stop-mark").await;
    assert!(
        read["stop_mark"].is_null(),
        "a refused mark was kept: {read}"
    );
}

#[tokio::test]
async fn a_reached_mark_leaves_the_queue_paused_until_resumed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = &harness.router;
    let (marked, _) = queued_download(router, "stop-mark-marked.bin").await;
    let (waiting, _) = queued_download(router, "stop-mark-waiting.bin").await;
    let (status, set) = common::put_json(
        router,
        "/api/v1/queue/stop-mark",
        serde_json::json!({ "download_id": marked }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{set}");

    // The parked scheduler starts nothing; the marked file is finished by hand.
    let id: rd_core::DownloadId = marked.parse().expect("id");
    for next in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        harness
            .database
            .transition_download(id, next)
            .await
            .expect("transition");
    }
    let mut pause = serde_json::Value::Null;
    for _ in 0..100 {
        pause = common::get_json(router, "/api/v1/queue/pause").await.1;
        if pause["stop_mark"].is_null() && pause["paused"] == true {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(pause["paused"], true, "{pause}");
    assert!(pause["until"].is_null(), "the pause has no end: {pause}");
    assert!(
        pause["stop_mark"].is_null(),
        "the mark was not cleared: {pause}"
    );
    assert_eq!(pause["files"], 1, "{pause}");

    let (status, resumed) = common::delete_json(router, "/api/v1/queue/pause").await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    assert_eq!(resumed["resumed"], 1, "{resumed}");
    let (_, downloads) = common::get_json(router, "/api/v1/downloads").await;
    let state = downloads
        .as_array()
        .expect("downloads")
        .iter()
        .find(|download| download["id"] == waiting)
        .and_then(|download| download["state"].as_str())
        .map(str::to_owned);
    assert_eq!(state.as_deref(), Some("queued"));
}
