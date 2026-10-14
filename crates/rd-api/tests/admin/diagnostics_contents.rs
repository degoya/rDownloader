//! The diagnostic bundle's `notifications.json` and `queue.json` over REST (RD-1291-04): a
//! package that finishes and is announced shows up by state, event and count, and nothing the
//! person named or wrote -- target, rule, template, package, folder, address -- is in the archive.

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use std::io::Read;

use axum::http::StatusCode;
use rd_core::{PackageId, PackageState};
use serde_json::json;

fn entry(archive: &mut zip::ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str) -> String {
    let mut text = String::new();
    archive
        .by_name(name)
        .expect("entry")
        .read_to_string(&mut text)
        .expect("read");
    text
}

#[tokio::test]
async fn an_announced_package_appears_by_state_and_count_and_never_by_name() {
    crate::diagnostics::data_directory();
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (_server, endpoint, _calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "target-name-canary", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, rule) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        json!({
            "name": "rule-name-canary",
            "target_id": target["id"],
            "events": ["package_completed"],
            "title_template": "title-template-canary {package}"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");

    let (status, created) = common::post_json(
        &router,
        "/api/v1/downloads",
        json!({
            "url": "https://example.test/url-canary.bin",
            "package_name": "package-name-canary"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let package: PackageId = created["package_id"]
        .as_str()
        .expect("package id")
        .parse()
        .expect("package id");
    let folder = harness
        .database
        .get_package(package)
        .await
        .expect("package")
        .expect("exists")
        .destination;
    assert!(!folder.is_empty());
    for state in [PackageState::Downloading, PackageState::Completed] {
        harness
            .database
            .set_package_state(package, state, None, None, None)
            .await
            .expect("state");
    }
    let database = &harness.database;
    common::eventually(
        std::time::Duration::from_secs(20),
        "the delivery was not recorded as delivered",
        || async move {
            let deliveries = database
                .list_notification_deliveries(10)
                .await
                .expect("deliveries");
            deliveries
                .iter()
                .any(|delivery| delivery.state == rd_notify::DeliveryState::Delivered)
                .then_some(())
        },
    )
    .await;

    let (_, preview) = common::get_json(&router, "/api/v1/diagnostics/bundle/preview").await;
    let (status, written) = common::post_json(
        &router,
        "/api/v1/diagnostics/bundle",
        json!({
            "approved": true,
            "digest": preview["digest"],
            "entries": ["notifications", "queue"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{written}");
    for path in ["notifications.json", "queue.json"] {
        assert!(
            written["manifest"]["redactions"]
                .as_array()
                .expect("redactions")
                .iter()
                .any(|line| line
                    .as_str()
                    .is_some_and(|line| line.starts_with(&format!("{path}: ")))),
            "{path}: {written}"
        );
    }
    let bytes = std::fs::read(written["path"].as_str().expect("path")).expect("bundle");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let notifications_text = entry(&mut archive, "notifications.json");
    let queue_text = entry(&mut archive, "queue.json");

    let notifications: serde_json::Value = serde_json::from_str(&notifications_text).expect("json");
    assert_eq!(notifications["targets"][0]["kind"], "webhook");
    assert_eq!(notifications["targets"][0]["id"], target["id"]);
    assert_eq!(
        notifications["rules"][0]["events"],
        json!(["package_completed"])
    );
    assert_eq!(notifications["rules"][0]["has_title_template"], true);
    assert_eq!(notifications["rules"][0]["has_body_template"], false);
    assert_eq!(
        notifications["delivery_counts"],
        json!([{ "event": "package_completed", "state": "delivered", "count": 1 }])
    );
    assert_eq!(notifications["recent_deliveries"][0]["attempts"], 1);
    assert_eq!(
        notifications["recent_deliveries"][0]["response_status"],
        200
    );

    let queue: serde_json::Value = serde_json::from_str(&queue_text).expect("json");
    assert_eq!(
        queue["packages_by_state"],
        json!([{ "value": "completed", "count": 1 }])
    );
    let transitions = queue["recent_transitions"].as_array().expect("transitions");
    assert_eq!(transitions[0]["previous"], "downloading", "{queue}");
    assert_eq!(transitions[0]["state"], "completed");
    assert_eq!(transitions[0]["package_id"], package.to_string());
    assert_eq!(transitions[1]["previous"], "queued");
    assert_eq!(transitions[1]["state"], "downloading");
    let files: u64 = queue["downloads"]
        .as_array()
        .expect("downloads")
        .iter()
        .filter_map(|row| row["count"].as_u64())
        .sum();
    assert_eq!(files, 1, "{queue}");

    for canary in [
        "target-name-canary",
        "rule-name-canary",
        "title-template-canary",
        "package-name-canary",
        "url-canary",
        endpoint.as_str(),
        folder.as_str(),
    ] {
        for (name, text) in [
            ("notifications.json", &notifications_text),
            ("queue.json", &queue_text),
        ] {
            assert!(!text.contains(canary), "{name} carries {canary}: {text}");
        }
    }
}
