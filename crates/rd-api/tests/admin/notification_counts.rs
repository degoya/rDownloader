//! The counted cases of RD-1260-04, end to end: the bus events a day produces, through the
//! notification hub, into queued deliveries. Each moment is one delivery -- a package written twice
//! and reordered afterwards, a storage root blocked and released, a wave of captchas, a power
//! countdown and its end -- where each event used to be one more.

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use axum::http::StatusCode;
use rd_core::{
    DownloadId, EventEnvelope, EventKind, Failure, FailureKind, PackageId, PackageState,
};
use rd_notify::NotificationEvent;
use serde_json::json;

/// A package with one download; answers both ids.
async fn package(harness: &common::Harness, name: &str) -> (PackageId, DownloadId) {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": format!("https://example.test/{name}.bin"), "package_name": name }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = |key: &str| created[key].as_str().expect("id").to_owned();
    (
        id("package_id").parse().expect("package id"),
        id("id").parse().expect("download id"),
    )
}

fn captcha(change: &str, waiting: usize) -> EventEnvelope {
    let pending: Vec<_> = (0..waiting)
        .map(|index| json!({ "id": format!("c{index}"), "kind": "image", "host": "files.example" }))
        .collect();
    EventEnvelope::new(
        EventKind::CaptchaChanged,
        json!({ "pending": pending, "change": change }),
    )
}

#[tokio::test]
async fn each_moment_of_a_day_is_one_delivery() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (endpoint, _calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, rule) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        json!({
            "name": "everything counted",
            "target_id": target["id"],
            "events": [
                "package_completed", "package_failed", "storage_blocked", "captcha_waiting",
                "power_pending"
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    let database = &harness.database;

    // A package finished, written once more, and its files reordered afterwards.
    let (finished, finished_file) = package(&harness, "Holiday").await;
    for _ in 0..2 {
        database
            .set_package_state(finished, PackageState::Completed, None, None, None)
            .await
            .expect("completed");
    }
    database
        .reorder_downloads(finished, vec![finished_file])
        .await
        .expect("reordered");
    // A package whose only file failed.
    let (_, broken) = package(&harness, "Broken").await;
    database
        .record_failure(
            broken,
            Failure::coded(FailureKind::Offline, "http.not_found", "gone"),
            None,
        )
        .await
        .expect("failed");
    // A storage root blocked and released again.
    for change in ["blocked", "released"] {
        database.broadcast(EventEnvelope::new(
            EventKind::StorageCapacity,
            json!({ "blocked": u8::from(change == "blocked"), "change": change }),
        ));
    }
    // Three captchas, one answered, two running out.
    for event in [
        captcha("added", 1),
        captcha("added", 2),
        captcha("added", 3),
        captcha("settled", 2),
        captcha("expired", 1),
        captcha("expired", 0),
    ] {
        database.broadcast(event);
    }
    // A countdown starting and cancelled.
    let runs_at = chrono::Utc::now() + chrono::Duration::seconds(60);
    database.broadcast(EventEnvelope::new(
        EventKind::PowerChanged,
        json!({ "state": "pending", "pending": "shutdown", "runs_at": runs_at }),
    ));
    database.broadcast(EventEnvelope::new(
        EventKind::PowerChanged,
        json!({ "state": "cancelled", "action": "shutdown", "reason": "cancelled" }),
    ));

    let expected = [
        (NotificationEvent::PackageCompleted, 1),
        (NotificationEvent::PackageFailed, 1),
        (NotificationEvent::StorageBlocked, 1),
        (NotificationEvent::CaptchaWaiting, 2),
        (NotificationEvent::PowerPending, 2),
    ];
    let total: usize = expected.iter().map(|(_, count)| count).sum();
    // Counted where they are queued: the sweep sends one per target every few seconds.
    common::eventually(
        std::time::Duration::from_secs(20),
        "the deliveries were not queued",
        || async move {
            let queued = database.list_notification_deliveries(100).await.ok()?;
            (queued.len() >= total).then_some(())
        },
    )
    .await;
    // Room for a late duplicate to show itself before counting.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let deliveries = database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries");
    for (event, count) in expected {
        let found: Vec<_> = deliveries
            .iter()
            .filter(|delivery| delivery.event == event)
            .map(|delivery| delivery.title.clone())
            .collect();
        assert_eq!(found.len(), count, "{event:?}: {found:?}");
    }
    assert_eq!(deliveries.len(), total, "{deliveries:?}");
    assert!(
        deliveries
            .iter()
            .any(|delivery| delivery.title == "Captcha not answered in time"),
        "{deliveries:?}"
    );
    assert!(
        deliveries
            .iter()
            .any(|delivery| delivery.title == "Shutdown cancelled"),
        "{deliveries:?}"
    );
}
