//! Round trips for the per-area bundles: subscriptions, streams, automations.
//!
//! The property that matters is that a file written on one instance means the same thing on
//! another, where none of the ids exist. So every test here exports, wipes nothing, and imports
//! into a *second* router with its own database.

mod common;

use axum::{Router, http::StatusCode};
use common::{get_json, post_json, test_router};
use serde_json::{Value, json};

async fn category(router: &Router, directory: &std::path::Path, name: &str) -> String {
    let storage_root_id = root(router, directory).await;
    let (status, created) = post_json(
        router,
        "/api/v1/categories",
        json!({ "name": name, "color": "#38BDF8", "storage_root_id": storage_root_id, "relative_path": "", "is_default": false }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("id").to_owned()
}

/// The first storage root, created on demand: a bare test router has none, and a category
/// cannot exist without one.
async fn root(router: &Router, directory: &std::path::Path) -> String {
    let (status, roots) = get_json(router, "/api/v1/storage-roots").await;
    assert_eq!(status, StatusCode::OK);
    if let Some(existing) = roots.as_array().expect("array").first() {
        return existing["id"].as_str().expect("id").to_owned();
    }
    let path = directory.join("downloads");
    std::fs::create_dir_all(&path).expect("downloads");
    let (status, created) = post_json(
        router,
        "/api/v1/storage-roots",
        json!({ "name": "Primary", "path": path.to_string_lossy(), "is_default": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("id").to_owned()
}

async fn export(router: &Router, area: &str) -> Value {
    let (status, bundle) = get_json(router, &format!("/api/v1/{area}/export")).await;
    assert_eq!(status, StatusCode::OK, "{bundle}");
    assert_eq!(bundle["format"], "rdownloader-area-bundle");
    assert_eq!(bundle["version"], 1);
    bundle
}

async fn import(router: &Router, area: &str, bundle: Value) -> (StatusCode, Value) {
    post_json(router, &format!("/api/v1/{area}/import"), bundle).await
}

#[tokio::test]
async fn subscriptions_travel_with_their_category_named_rather_than_referenced() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    let source = test_router(source_dir.path()).await;
    let category_id = category(&source, source_dir.path(), "Music").await;
    let (status, created) = post_json(
        &source,
        "/api/v1/subscriptions",
        json!({
            "name": "Weekly", "url": "https://example.test/feed.xml", "kind": "feed",
            "enabled": true, "mode": "review", "interval_seconds": 3_600,
            "category_id": category_id, "filters": {}, "backlog": { "mode": "from_now" },
            "source_categories": ["3010"], "card_ratio": "1:1"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let bundle = export(&source, "subscriptions").await;
    let entry = &bundle["subscriptions"].as_array().expect("array")[0];
    assert_eq!(entry["category_name"], "Music");
    assert!(entry.get("category_id").is_none(), "ids do not travel");
    assert_eq!(entry["source_categories"], json!(["3010"]));
    assert_eq!(entry["card_ratio"], "1:1");

    // A second instance, with a category of the same name and nothing else in common.
    let target_dir = tempfile::tempdir().expect("tempdir");
    let target = test_router(target_dir.path()).await;
    category(&target, target_dir.path(), "Music").await;
    let (status, summary) = import(&target, "subscriptions", bundle.clone()).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["created"], 1);
    assert_eq!(summary["skipped"], 0);

    let (_, listed) = get_json(&target, "/api/v1/subscriptions").await;
    let restored = &listed.as_array().expect("array")[0];
    assert_eq!(restored["name"], "Weekly");
    assert_eq!(restored["source_categories"], json!(["3010"]));
    assert_eq!(
        restored["card_ratio"], "1:1",
        "the card ratio travels (RD-120-42)"
    );
    // Attached to the target's own category of that name, not to the source's id.
    assert_ne!(restored["category_id"], Value::Null);

    // Re-importing the same file changes nothing.
    let (status, summary) = import(&target, "subscriptions", bundle).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["created"], 0);
    assert_eq!(summary["skipped"], 1);
}

/// RD-130-19: a script subscription never travels in a bundle. A bundle is a file somebody
/// sends, and a script entry in it would be an instruction to run code on the machine that
/// opens it: the export leaves it out, and the import skips one a hand-written file carries.
#[tokio::test]
async fn a_script_subscription_neither_leaves_nor_enters_through_a_bundle() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    let source = test_router(source_dir.path()).await;
    let scripts = source_dir.path().join("scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    std::fs::write(
        scripts.join("daily-links.sh"),
        "echo https://example.test/a.rar\n",
    )
    .expect("script");
    for request in [
        json!({ "name": "Weekly", "url": "https://example.test/feed.xml", "kind": "feed",
                "interval_seconds": 3_600 }),
        json!({ "name": "Daily links", "url": "script:daily-links.sh", "kind": "script",
                "interval_seconds": 3_600, "schedule": "0 6 * * *" }),
    ] {
        let (status, created) = post_json(&source, "/api/v1/subscriptions", request).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
    }

    let mut bundle = export(&source, "subscriptions").await;
    let entries = bundle["subscriptions"].as_array().expect("array").clone();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["name"], "Weekly");

    // A hand-written file that carries one anyway.
    let mut script = entries[0].clone();
    script["name"] = json!("Smuggled");
    script["kind"] = json!("script");
    script["url"] = json!("script:daily-links.sh");
    bundle["subscriptions"]
        .as_array_mut()
        .expect("array")
        .push(script);
    let target_dir = tempfile::tempdir().expect("tempdir");
    let target = test_router(target_dir.path()).await;
    let (status, summary) = import(&target, "subscriptions", bundle).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(
        (summary["created"].as_u64(), summary["skipped"].as_u64()),
        (Some(1), Some(1))
    );
    let (_, listed) = get_json(&target, "/api/v1/subscriptions").await;
    let kinds: Vec<&str> = listed
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|entry| entry["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["feed"], "{listed}");
}

/// A subscription that needed a key arrives switched off, because the key is not in the file.
#[tokio::test]
async fn a_subscription_that_needs_a_key_arrives_disabled() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    let source = test_router(source_dir.path()).await;
    let (status, created) = post_json(
        &source,
        "/api/v1/subscriptions",
        json!({
            "name": "Indexer", "url": "https://indexer.test/api", "kind": "indexer",
            "enabled": true, "mode": "review", "interval_seconds": 3_600,
            "filters": {}, "backlog": { "mode": "from_now" }, "api_key": "super-secret"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let bundle = export(&source, "subscriptions").await;
    let entry = &bundle["subscriptions"].as_array().expect("array")[0];
    assert_eq!(entry["api_key_required"], true);
    // The one property this format has to hold: no credential is in the file.
    assert!(
        !serde_json::to_string(&bundle)
            .expect("json")
            .contains("super-secret"),
        "the bundle carries the key"
    );

    let target_dir = tempfile::tempdir().expect("tempdir");
    let target = test_router(target_dir.path()).await;
    let (status, summary) = import(&target, "subscriptions", bundle).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["created"], 1);

    let (_, listed) = get_json(&target, "/api/v1/subscriptions").await;
    let restored = &listed.as_array().expect("array")[0];
    assert_eq!(restored["enabled"], false, "it would poll without a key");
    assert_eq!(restored["has_secret"], false);
}

#[tokio::test]
async fn stream_schedules_reattach_to_their_channel_by_name() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    let source = test_router(source_dir.path()).await;
    let (status, channel) = post_json(
        &source,
        "/api/v1/streams/channels",
        json!({ "url": "https://example.test/live", "name": "Live Channel", "quality": "best", "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{channel}");
    let channel_id = channel["id"].as_str().expect("id");
    let (status, schedule) = post_json(
        &source,
        "/api/v1/streams/schedules",
        json!({
            "channel_id": channel_id, "name": "Evenings", "enabled": true,
            "kind": "weekly", "days": [1], "start_minute": 1_200,
            "timezone": "Europe/Berlin", "window_minutes": 60
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{schedule}");

    let bundle = export(&source, "streams").await;
    assert_eq!(
        bundle["stream_channels"].as_array().expect("array").len(),
        1
    );
    let entry = &bundle["stream_schedules"].as_array().expect("array")[0];
    assert_eq!(entry["channel_name"], "Live Channel");
    assert_eq!(entry["timezone"], "Europe/Berlin");

    let target_dir = tempfile::tempdir().expect("tempdir");
    let target = test_router(target_dir.path()).await;
    let (status, summary) = import(&target, "streams", bundle).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    // The channel and the schedule, the schedule attached to the channel just created.
    assert_eq!(summary["created"], 2);

    let (_, schedules) = get_json(&target, "/api/v1/streams/schedules").await;
    let restored = &schedules.as_array().expect("array")[0];
    assert_eq!(restored["name"], "Evenings");
    assert_eq!(restored["timezone"], "Europe/Berlin");
}

/// An action pointing at something the target does not have cannot be carried over, and half an
/// automation is not the automation somebody exported.
#[tokio::test]
async fn an_automation_whose_action_cannot_be_resolved_is_skipped_whole() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    let source = test_router(source_dir.path()).await;
    let category_id = category(&source, source_dir.path(), "Films").await;
    let (status, created) = post_json(
        &source,
        "/api/v1/automations",
        json!({
            "name": "Sort films", "enabled": true, "trigger": "download_completed",
            "actions": [{ "kind": "set_category", "category_id": category_id }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let bundle = export(&source, "automations").await;
    let entry = &bundle["automations"].as_array().expect("array")[0];
    assert_eq!(entry["actions"][0]["kind"], "set_category");
    assert_eq!(entry["actions"][0]["category_name"], "Films");

    // No category of that name here.
    let bare_dir = tempfile::tempdir().expect("tempdir");
    let bare = test_router(bare_dir.path()).await;
    let (status, summary) = import(&bare, "automations", bundle.clone()).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["created"], 0);
    assert_eq!(summary["skipped"], 1);
    let (_, listed) = get_json(&bare, "/api/v1/automations").await;
    assert!(listed.as_array().expect("array").is_empty());

    // With the category present it resolves and comes across.
    let ready_dir = tempfile::tempdir().expect("tempdir");
    let ready = test_router(ready_dir.path()).await;
    category(&ready, ready_dir.path(), "Films").await;
    let (status, summary) = import(&ready, "automations", bundle).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["created"], 1);
}

#[tokio::test]
async fn a_bundle_for_another_area_is_refused_rather_than_silently_ignored() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let streams = export(&router, "streams").await;

    let (status, problem) = import(&router, "subscriptions", streams).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "backup.area_missing");
}

#[tokio::test]
async fn a_foreign_or_newer_bundle_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;

    let (status, problem) = import(
        &router,
        "subscriptions",
        json!({ "format": "something-else", "version": 1, "exported_at": "2026-09-07T00:00:00Z", "app_version": "1.0.1", "subscriptions": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "backup.format_unsupported");

    let (status, problem) = import(
        &router,
        "subscriptions",
        json!({ "format": "rdownloader-area-bundle", "version": 99, "exported_at": "2026-09-07T00:00:00Z", "app_version": "9.9.9", "subscriptions": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "backup.version_unsupported");
}
