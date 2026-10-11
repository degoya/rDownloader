//! The finished package's folder on its notification (RD-1250-01): a package that completes
//! queues its deliveries with the folder it finished in, so a media server is asked to scan
//! that folder -- on the first attempt and on every retry -- and a failed package names none.

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use axum::http::StatusCode;
use rd_core::{PackageId, PackageState};
use serde_json::json;

/// A package with one download, by its name; answers its id.
async fn package(harness: &common::Harness, name: &str) -> PackageId {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": format!("https://example.test/{name}.bin"), "package_name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["package_id"]
        .as_str()
        .expect("package id")
        .parse()
        .expect("package id")
}

#[tokio::test]
async fn a_finished_package_queues_its_folder_and_a_failed_one_none() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, rule) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        json!({
            "name": "packages",
            "target_id": target["id"],
            "events": ["package_completed", "package_failed"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");

    let finished = package(&harness, "Holiday").await;
    let failed = package(&harness, "Broken").await;
    let folder = harness
        .database
        .get_package(finished)
        .await
        .expect("package")
        .expect("exists")
        .destination;
    assert!(!folder.is_empty());
    for (id, state) in [
        (finished, PackageState::Completed),
        (failed, PackageState::Failed),
    ] {
        harness
            .database
            .set_package_state(id, state, None, None, None)
            .await
            .expect("state");
    }

    let received = &calls;
    common::eventually(
        std::time::Duration::from_secs(20),
        "the two deliveries did not arrive",
        || async move { (received.lock().expect("calls").len() >= 2).then_some(()) },
    )
    .await;
    let deliveries = harness
        .database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries");
    assert_eq!(deliveries.len(), 2, "{deliveries:?}");
    let path_of = |event| {
        deliveries
            .iter()
            .find(|delivery| delivery.event == event)
            .map(|delivery| delivery.package_path.clone())
    };
    assert_eq!(
        path_of(rd_notify::NotificationEvent::PackageCompleted),
        Some(Some(folder))
    );
    assert_eq!(
        path_of(rd_notify::NotificationEvent::PackageFailed),
        Some(None)
    );
    // Kept with the delivery, never shown: the history answers without it.
    let (status, history) = common::get_json(&router, "/api/v1/notifications/deliveries").await;
    assert_eq!(status, StatusCode::OK, "{history}");
    assert!(
        history
            .as_array()
            .expect("list")
            .iter()
            .all(|delivery| delivery.get("package_path").is_none()),
        "{history}"
    );
}
