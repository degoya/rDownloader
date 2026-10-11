//! The clean-up of the data directory over REST (RD-1240-34): the preview names what goes and
//! removes nothing, an unconfirmed clean-up is refused, a confirmed one keeps only the newest
//! copy of each kind behind a proven update and writes itself into the audit log; behind a
//! journal that does not say `verified` everything stays. The retention rules themselves are
//! measured in `rd-backup` (`update_retention_tests.rs`), the plugin cache in `rd-plugin-host`.
//!
//! The database's part (RD-1240-35): old skipped or dismissed subscription items are compacted
//! to their keys unless the retention is 0, and the file of a fresh installation is incremental,
//! so it is never rewritten. The compaction and the free pages are measured in `rd-db`
//! (`tests/database/database_growth.rs`).

use crate::common;

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use common::{get_json, post_json, put_json, test_harness};
use serde_json::json;

/// A copy before an update from `from` to `to`, named and placed as the preparation writes it.
fn copy(data: &Path, from: &str, to: &str, day: u32) -> PathBuf {
    let folder = data.join(rd_backup::pre_update::DIRECTORY);
    std::fs::create_dir_all(&folder).expect("folder");
    let at = Utc
        .with_ymd_and_hms(2026, 10, day, 12, 0, 0)
        .single()
        .expect("time");
    let path = folder.join(rd_db::pre_migration::copy_name(from, to, at));
    std::fs::write(&path, vec![0_u8; 1000]).expect("copy");
    path
}

#[tokio::test]
async fn a_confirmed_clean_up_keeps_the_newest_copy_behind_a_proven_update() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let older = copy(directory.path(), "1.22.0", "1.23.0", 1);
    let newest = copy(directory.path(), "1.23.0", "1.24.0", 2);

    let (status, preview) = get_json(&harness.router, "/api/v1/system/cleanup").await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(
        preview["update_proven"], true,
        "no update recorded: {preview}"
    );
    assert_eq!(preview["retention_days"], 14);
    assert_eq!(preview["pre_update"]["removable_files"], 1, "{preview}");
    assert_eq!(preview["pre_update"]["removable_bytes"], 1000);
    assert_eq!(preview["pre_update"]["kept_files"], 1);
    assert!(older.exists(), "a preview removes nothing");

    let (status, refused) = post_json(&harness.router, "/api/v1/system/cleanup", json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "data_reset.not_confirmed");
    assert!(older.exists());

    let (status, done) = post_json(
        &harness.router,
        "/api/v1/system/cleanup",
        json!({ "confirmed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(
        done["pre_update"]["removable_files"], 1,
        "what went: {done}"
    );
    assert!(!older.exists());
    assert!(newest.exists(), "the newest copy is what a rollback needs");

    let (status, records) = get_json(
        &harness.router,
        "/api/v1/audit/records?action=system_cleanup",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{records}");
    let entry = &records["records"][0];
    assert_eq!(entry["details"]["removed_bytes"], "1000", "{records}");
}

#[tokio::test]
async fn a_journal_that_does_not_say_verified_keeps_every_copy() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let older = copy(directory.path(), "1.22.0", "1.23.0", 1);
    copy(directory.path(), "1.23.0", "1.24.0", 2);
    // Not a whole journal, so it does not read; what does not read proves no update, exactly
    // like a `switched` one (`rd_update`'s own tests cover every phase).
    let update = directory.path().join(rd_update::install::UPDATE_DIR);
    std::fs::create_dir_all(&update).expect("update folder");
    std::fs::write(
        update.join(rd_update::install::JOURNAL_FILE),
        b"{ \"phase\": \"switched\" }",
    )
    .expect("journal");

    let (status, done) = post_json(
        &harness.router,
        "/api/v1/system/cleanup",
        json!({ "confirmed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["update_proven"], false, "{done}");
    assert_eq!(done["pre_update"]["removable_files"], 0);
    assert_eq!(done["pre_update"]["kept_files"], 2);
    assert!(older.exists());
}

/// A subscription with `count` skipped items, all discovered 120 days ago.
async fn aged_archive(harness: &common::Harness, count: usize) {
    use sqlx::Connection;

    let request = json!({
        "name": "Indexer",
        "url": "https://example.test/c/channel",
        "kind": "media",
        "enabled": false,
        "mode": "review",
        "interval_seconds": 3_600,
        "filters": {},
        "backlog": { "mode": "from_now" }
    });
    let (status, created) = post_json(&harness.router, "/api/v1/subscriptions", request).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("subscription id");
    let items = (0..count)
        .map(|index| rd_db::NewSubscriptionItem {
            item_key: format!("id:{index}"),
            title: format!("Some.Release.{index}"),
            url: format!("https://example.test/get/{index}.nzb")
                .parse()
                .expect("url"),
            published_at: None,
            duration_seconds: None,
            state: rd_core::SubscriptionItemState::Skipped,
            reason: None,
            source_category: None,
            media_type: None,
            attributes: std::collections::BTreeMap::new(),
            password: None,
        })
        .collect();
    harness
        .database
        .record_subscription_items(id, items)
        .await
        .expect("items");
    let mut connection =
        sqlx::SqliteConnection::connect(&format!("sqlite://{}", harness.database_path.display()))
            .await
            .expect("connect");
    sqlx::query("UPDATE subscription_items SET discovered_at = ?")
        .bind(Utc::now() - chrono::Duration::days(120))
        .execute(&mut connection)
        .await
        .expect("age");
}

#[tokio::test]
async fn a_clean_up_compacts_the_old_archive_and_never_rewrites_a_fresh_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    aged_archive(&harness, 3).await;

    let (status, preview) = get_json(&harness.router, "/api/v1/system/cleanup").await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let database = &preview["database"];
    assert_eq!(database["incremental"], true, "{preview}");
    assert_eq!(database["item_retention_days"], 30);
    assert_eq!(database["item_rows"], 3);
    assert_eq!(
        database["compactable_items"], 3,
        "a preview counts: {preview}"
    );
    assert_eq!(database["rewrite_refused"], serde_json::Value::Null);

    let (status, done) = post_json(
        &harness.router,
        "/api/v1/system/cleanup",
        json!({ "confirmed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["database"]["compactable_items"], 3, "{done}");
    assert_eq!(done["database"]["item_rows"], 0);
    assert_eq!(done["database"]["item_key_rows"], 3, "the keys stay");
    assert_eq!(done["database"]["rewrite_refused"], serde_json::Value::Null);
}

#[tokio::test]
async fn a_retention_of_zero_keeps_every_archived_item_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    aged_archive(&harness, 2).await;
    let (_, mut settings) = get_json(&harness.router, "/api/v1/settings").await;
    // As in `updates`: the harness runs with the login off.
    settings["admin_login_disabled"] = json!(true);
    settings["subscription_item_retention_days"] = json!(0);
    let (status, saved) = put_json(&harness.router, "/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    settings["subscription_item_retention_days"] = json!(3651);
    let (status, refused) = put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(
        refused["code"],
        "settings.subscription_item_retention_invalid"
    );

    let (status, done) = post_json(
        &harness.router,
        "/api/v1/system/cleanup",
        json!({ "confirmed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["database"]["item_retention_days"], 0, "{done}");
    assert_eq!(done["database"]["compactable_items"], 0);
    assert_eq!(done["database"]["item_rows"], 2);
}
