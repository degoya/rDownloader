//! Integration tests for the unified capacity check and the per-root intake stop
//! (RD-050-15): a root below its threshold takes no new work, every other destination
//! keeps accepting links, and the release path requeues exactly what was held back.

mod common;

use axum::{Router, http::StatusCode};

/// A threshold no temporary directory can ever satisfy, so the supervision loop blocks the
/// root without the test having to fill a disk.
const UNREACHABLE_THRESHOLD: u64 = 1 << 62;

async fn create_root(router: &Router, name: &str, path: &std::path::Path) -> String {
    let (status, root) = common::post_json(
        router,
        "/api/v1/storage-roots",
        serde_json::json!({
            "name": name,
            "path": path.to_string_lossy(),
            "is_default": false,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{root}");
    root["id"].as_str().expect("root id").to_owned()
}

async fn create_category(router: &Router, name: &str, root_id: &str) -> String {
    let (status, category) = common::post_json(
        router,
        "/api/v1/categories",
        serde_json::json!({
            "name": name,
            "color": "#336699",
            "storage_root_id": root_id,
            "relative_path": name,
            "is_default": false,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{category}");
    category["id"].as_str().expect("category id").to_owned()
}

/// Adds one link and returns its collector package id.
async fn collect_link(router: &Router, url: &str) -> String {
    let (status, batch) = common::post_json(
        router,
        "/api/v1/collector/batches",
        serde_json::json!({ "source": "manual", "links": [{ "url": url }] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{batch}");
    batch["packages"][0]["id"]
        .as_str()
        .expect("package id")
        .to_owned()
}

/// Waits for the supervision loop to reach `blocked` for a root.
async fn wait_for_blocked(router: &Router, root_id: &str, blocked: bool) -> serde_json::Value {
    for _ in 0..120 {
        let (status, capacity) = common::get_json(router, "/api/v1/storage/capacity").await;
        assert_eq!(status, StatusCode::OK, "{capacity}");
        let entry = capacity["roots"]
            .as_array()
            .expect("roots")
            .iter()
            .find(|entry| entry["storage_root_id"] == root_id)
            .cloned();
        if let Some(entry) = entry
            && entry["blocked"] == blocked
        {
            return entry;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("storage root never reached blocked={blocked}");
}

async fn set_threshold(
    router: &Router,
    root_id: &str,
    path: &std::path::Path,
    minimum: Option<u64>,
) {
    let (status, updated) = common::put_json(
        router,
        &format!("/api/v1/storage-roots/{root_id}"),
        serde_json::json!({
            "name": path.file_name().expect("name").to_string_lossy(),
            "path": path.to_string_lossy(),
            "is_default": false,
            "minimum_free_bytes": minimum.map(|value| value.to_string()),
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
}

#[tokio::test]
async fn a_blocked_root_refuses_intake_while_other_destinations_keep_working() {
    let directory = tempfile::tempdir().expect("tempdir");
    let full = directory.path().join("full");
    let spacious = directory.path().join("spacious");
    let router = common::test_router(directory.path()).await;

    let full_id = create_root(&router, "full", &full).await;
    let spacious_id = create_root(&router, "spacious", &spacious).await;
    let full_category = create_category(&router, "movies", &full_id).await;
    let spacious_category = create_category(&router, "shows", &spacious_id).await;

    set_threshold(&router, &full_id, &full, Some(UNREACHABLE_THRESHOLD)).await;
    let entry = wait_for_blocked(&router, &full_id, true).await;
    assert_eq!(entry["shortfall"]["size_known"], true);

    let blocked_package = collect_link(&router, "https://files.example.com/blocked.bin").await;
    let allowed_package = collect_link(&router, "https://files.example.com/allowed.bin").await;
    common::wait_for_candidates_ready(&router).await;
    for (package, category) in [
        (&blocked_package, &full_category),
        (&allowed_package, &spacious_category),
    ] {
        let (status, updated) = common::patch_json(
            &router,
            &format!("/api/v1/collector/packages/{package}"),
            serde_json::json!({ "category_id": category }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
    }

    let (status, refused) = common::post_json(
        &router,
        &format!("/api/v1/collector/packages/{blocked_package}/enqueue"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "storage.capacity_blocked");

    // The other root is untouched: a full media disk must not stop everything else.
    let (status, enqueued) = common::post_json(
        &router,
        &format!("/api/v1/collector/packages/{allowed_package}/enqueue"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{enqueued}");
}

#[tokio::test]
async fn a_root_is_released_again_once_its_threshold_fits() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root_path = directory.path().join("data");
    let router = common::test_router(directory.path()).await;
    let root_id = create_root(&router, "data", &root_path).await;
    let category = create_category(&router, "files", &root_id).await;

    set_threshold(&router, &root_id, &root_path, Some(UNREACHABLE_THRESHOLD)).await;
    wait_for_blocked(&router, &root_id, true).await;

    set_threshold(&router, &root_id, &root_path, None).await;
    let entry = wait_for_blocked(&router, &root_id, false).await;
    assert!(entry["shortfall"].is_null());

    let package = collect_link(&router, "https://files.example.com/after-release.bin").await;
    common::wait_for_candidates_ready(&router).await;
    let (status, updated) = common::patch_json(
        &router,
        &format!("/api/v1/collector/packages/{package}"),
        serde_json::json!({ "category_id": category }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    let (status, enqueued) = common::post_json(
        &router,
        &format!("/api/v1/collector/packages/{package}/enqueue"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{enqueued}");
}

#[tokio::test]
async fn without_automatic_resume_a_root_stays_blocked_until_it_is_released() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root_path = directory.path().join("data");
    let harness = common::test_harness(directory.path()).await;
    let router = harness.router.clone();
    // The supervision loop reads the settings blob every cycle, so writing it directly is
    // enough here and keeps the test out of the admin-login flow.
    harness
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "storage_auto_resume": false }),
        )
        .await
        .expect("settings");
    let root_id = create_root(&router, "data", &root_path).await;

    set_threshold(&router, &root_id, &root_path, Some(UNREACHABLE_THRESHOLD)).await;
    wait_for_blocked(&router, &root_id, true).await;

    set_threshold(&router, &root_id, &root_path, None).await;
    // Two supervision cycles are far more than the loop needs; the block must survive them.
    tokio::time::sleep(std::time::Duration::from_millis(2_500)).await;
    let (_, capacity) = common::get_json(&router, "/api/v1/storage/capacity").await;
    let entry = capacity["roots"]
        .as_array()
        .expect("roots")
        .iter()
        .find(|entry| entry["storage_root_id"] == root_id)
        .cloned()
        .expect("root");
    assert_eq!(entry["blocked"], true, "{capacity}");

    let (status, released) = common::post_json(
        &router,
        &format!("/api/v1/storage/capacity/{root_id}/resume"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{released}");
    assert_eq!(released["code"], "storage.resumed");
    wait_for_blocked(&router, &root_id, false).await;
}

#[tokio::test]
async fn a_block_survives_a_restart_when_automatic_resume_is_off() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root_path = directory.path().join("data");
    let root_id = {
        let harness = common::test_harness(directory.path()).await;
        harness
            .database
            .set_setting(
                "service.settings".to_owned(),
                serde_json::json!({ "storage_auto_resume": false }),
            )
            .await
            .expect("settings");
        let root_id = create_root(&harness.router, "data", &root_path).await;
        set_threshold(
            &harness.router,
            &root_id,
            &root_path,
            Some(UNREACHABLE_THRESHOLD),
        )
        .await;
        wait_for_blocked(&harness.router, &root_id, true).await;
        // The threshold is lowered before the restart, so only the persisted block can
        // still be holding the root back afterwards.
        set_threshold(&harness.router, &root_id, &root_path, None).await;
        root_id
    };

    let restarted = common::test_harness(directory.path()).await;
    let (status, capacity) = common::get_json(&restarted.router, "/api/v1/storage/capacity").await;
    assert_eq!(status, StatusCode::OK, "{capacity}");
    let entry = capacity["roots"]
        .as_array()
        .expect("roots")
        .iter()
        .find(|entry| entry["storage_root_id"] == root_id)
        .cloned()
        .expect("root");
    assert_eq!(entry["blocked"], true, "{capacity}");
}
