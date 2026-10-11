//! The activity events of the notification hub (RD-1240-17): links arriving, downloads
//! starting, a recording finishing and a subscription check accepting items. A burst of
//! imports or starts reaches a rule as one notification, and the two chatty events reach only
//! a rule that lists them.

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use std::time::Duration;

use axum::http::StatusCode;
use rd_core::{EventEnvelope, EventKind};
use serde_json::json;

/// A burst closes ten seconds after its last occurrence, and the queue is swept every five.
const BURST: Duration = Duration::from_secs(40);

/// Creates a rule for `events` on `target` and returns its id.
async fn rule(router: &axum::Router, target: &serde_json::Value, events: &[&str]) -> String {
    let (status, rule) = common::post_json(
        router,
        "/api/v1/notifications/rules",
        json!({
            "name": format!("for [{}]", events.join(", ")),
            "target_id": target["id"],
            "events": events
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    rule["id"].as_str().expect("id").to_owned()
}

#[tokio::test]
async fn a_burst_of_imports_reaches_a_rule_as_one_notification() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = harness.router.clone();
    let (endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    rule(&router, &target, &["links_added"]).await;

    for batch in 0..25 {
        harness.database.broadcast(EventEnvelope::new(
            EventKind::CollectorIntake,
            json!({
                "batch_id": format!("batch-{batch}"),
                "candidate_count": 4,
                "package_count": 1,
                "source": "clipboard"
            }),
        ));
    }

    let received = &calls;
    let (_, _, body) =
        common::eventually(BURST, "no links_added delivery arrived", || async move {
            received.lock().expect("calls").first().cloned()
        })
        .await;
    let body: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(body["event"], "links_added", "{body}");
    assert_eq!(body["title"], "100 links added in 25 imports", "{body}");
    assert!(
        body["body"]
            .as_str()
            .is_some_and(|text| text.contains("from: clipboard")),
        "{body}"
    );
    let deliveries = harness
        .database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries");
    assert_eq!(deliveries.len(), 1, "{deliveries:?}");
}

#[tokio::test]
async fn a_recording_starts_for_the_rule_that_lists_it_and_finishes_for_every_rule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (endpoint, _calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let starts = rule(&router, &target, &["download_started"]).await;
    let every = rule(&router, &target, &[]).await;

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Channel-20261010".to_owned(),
            destination: directory.path().join("out").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = harness
        .database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id: package.id,
            source: "https://example.com/channel".parse().expect("url"),
            file_name: "Channel-20261010.ts".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Record,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    for next in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Downloading,
        rd_core::DownloadState::Verifying,
        rd_core::DownloadState::Completed,
    ] {
        harness
            .database
            .transition_download(download.id, next)
            .await
            .expect("transition");
    }

    let database = &harness.database;
    let wanted = [
        rd_notify::NotificationEvent::DownloadStarted,
        rd_notify::NotificationEvent::StreamRecorded,
    ];
    let deliveries = common::eventually(
        BURST,
        "start and recording were not queued",
        || async move {
            let deliveries = database
                .list_notification_deliveries(100)
                .await
                .expect("deliveries");
            wanted
                .iter()
                .all(|event| deliveries.iter().any(|delivery| delivery.event == *event))
                .then_some(deliveries)
        },
    )
    .await;
    // The package finishing reaches the rule without an event list as well; what matters is
    // who got the start and who got the recording.
    let of = |event: rd_notify::NotificationEvent| -> Vec<(String, String)> {
        deliveries
            .iter()
            .filter(|delivery| delivery.event == event)
            .map(|delivery| (delivery.rule_id.to_string(), delivery.title.clone()))
            .collect()
    };
    assert_eq!(
        of(rd_notify::NotificationEvent::DownloadStarted),
        vec![(starts, "Download started: Channel-20261010.ts".to_owned())],
        "{deliveries:?}"
    );
    assert_eq!(
        of(rd_notify::NotificationEvent::StreamRecorded),
        vec![(every, "Stream recorded: Channel-20261010.ts".to_owned())],
        "{deliveries:?}"
    );
}

#[tokio::test]
async fn only_a_subscription_check_that_accepted_items_notifies() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = harness.router.clone();
    let (endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    rule(&router, &target, &["subscription_matched"]).await;

    let subscription = rd_core::SubscriptionId::new().to_string();
    for payload in [
        json!({ "resource": "subscription" }),
        json!({
            "resource": "subscription", "poll": "finished", "subscription_id": subscription,
            "found": 4, "accepted": 0, "skipped": 4, "error": null
        }),
        json!({
            "resource": "subscription", "poll": "finished", "subscription_id": subscription,
            "found": 3, "accepted": 2, "skipped": 1, "error": null
        }),
    ] {
        harness
            .database
            .broadcast(EventEnvelope::new(EventKind::SubscriptionChanged, payload));
    }

    let received = &calls;
    let (_, _, body) = common::eventually(
        Duration::from_secs(20),
        "no subscription_matched delivery arrived",
        || async move { received.lock().expect("calls").first().cloned() },
    )
    .await;
    let body: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(body["event"], "subscription_matched", "{body}");
    assert_eq!(
        body["title"], "New in A subscription: 2 new items",
        "{body}"
    );
    let deliveries = harness
        .database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries");
    assert_eq!(deliveries.len(), 1, "{deliveries:?}");
}
