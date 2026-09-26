//! One contract test per trigger (RD-090-06).
//!
//! Each trigger is driven by the event that is supposed to produce it and by nothing else,
//! so a trigger that quietly stops firing — because a payload key was renamed, or a state
//! transition changed shape — fails here instead of becoming an automation that never runs.

mod common;

use common::{get_json, parked_harness, post_json, test_harness};
use rd_core::{EventEnvelope, EventKind};
use serde_json::json;

/// Creates an enabled automation listening for one trigger, and returns its id.
async fn automation_for(router: &axum::Router, trigger: &str) -> String {
    let (status, created) = post_json(
        router,
        "/api/v1/automations",
        json!({
            "name": format!("Listening for {trigger}"),
            "enabled": true,
            "trigger": trigger,
            "condition": { "type": "always" },
            "actions": [{ "kind": "pause_package" }]
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("id").to_owned()
}

/// Waits until a run exists for the given automation.
async fn fired(router: &axum::Router, automation_id: &str) -> bool {
    for _ in 0..40 {
        let (_, runs) = get_json(
            router,
            &format!("/api/v1/automations/runs?automation_id={automation_id}"),
        )
        .await;
        if runs.as_array().is_some_and(|items| !items.is_empty()) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn the_download_lifecycle_triggers_fire_on_a_real_job() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;

    let resolved = automation_for(&harness.router, "download_resolved").await;
    let started = automation_for(&harness.router, "download_started").await;
    let completed = automation_for(&harness.router, "download_completed").await;
    let package_completed = automation_for(&harness.router, "package_completed").await;

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Lifecycle.Release".to_owned(),
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
        .create_download(common_download(package.id, "Lifecycle.Release.mkv"))
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

    for (id, name) in [
        (&resolved, "download_resolved"),
        (&started, "download_started"),
        (&completed, "download_completed"),
        (&package_completed, "package_completed"),
    ] {
        assert!(fired(&harness.router, id).await, "{name} did not fire");
    }
}

#[tokio::test]
async fn a_failed_download_fires_the_failure_triggers() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;

    let failed = automation_for(&harness.router, "download_failed").await;
    let package_failed = automation_for(&harness.router, "package_failed").await;

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Failing.Release".to_owned(),
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
        .create_download(common_download(package.id, "Failing.Release.mkv"))
        .await
        .expect("download");
    for next in [
        rd_core::DownloadState::Resolving,
        rd_core::DownloadState::Failed,
    ] {
        harness
            .database
            .transition_download(download.id, next)
            .await
            .expect("transition");
    }

    assert!(
        fired(&harness.router, &failed).await,
        "download_failed did not fire"
    );

    // A failed download does not fail its package: `package_failed` means post-processing
    // failed, which is a different moment and deliberately not implied by a dead link. It is
    // driven directly here rather than by running a real repair.
    harness
        .database
        .set_package_state(package.id, rd_core::PackageState::Failed, None, None, None)
        .await
        .expect("package state");
    assert!(
        fired(&harness.router, &package_failed).await,
        "package_failed did not fire"
    );
}

#[tokio::test]
async fn the_remaining_triggers_fire_on_their_own_event() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    // Post-processing, storage and subscription moments are broadcast by parts of the
    // service a router test cannot drive end to end, so the event itself stands in. What is
    // being checked is the translation from payload to trigger, which is where it breaks.
    let cases: Vec<(&str, EventKind, serde_json::Value)> = vec![
        (
            "intake_received",
            EventKind::CollectorIntake,
            json!({ "batch_id": "b", "candidate_count": 1, "source": "api" }),
        ),
        (
            "extraction_finished",
            EventKind::PostprocessProgress,
            json!({ "owner_id": "o", "kind": "extract_rar", "state": "completed" }),
        ),
        (
            "script_finished",
            EventKind::PostprocessProgress,
            json!({ "owner_id": "o", "kind": "script", "state": "completed" }),
        ),
        (
            "upload_finished",
            EventKind::PostprocessProgress,
            json!({ "owner_id": "o", "kind": "upload", "state": "completed" }),
        ),
        (
            "storage_threshold",
            EventKind::StorageCapacity,
            json!({ "target": "root", "blocked": true }),
        ),
        (
            "subscription_item",
            EventKind::SubscriptionChanged,
            json!({ "accepted_items": 1, "name": "A feed" }),
        ),
    ];

    let mut ids = Vec::new();
    for (trigger, _, _) in &cases {
        ids.push(automation_for(&harness.router, trigger).await);
    }
    for (_, kind, payload) in &cases {
        harness
            .database
            .broadcast(EventEnvelope::new(kind.clone(), payload.clone()));
    }
    for ((trigger, _, _), id) in cases.iter().zip(&ids) {
        assert!(fired(&harness.router, id).await, "{trigger} did not fire");
    }
}

#[tokio::test]
async fn an_event_that_is_not_a_moment_fires_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let mut ids = Vec::new();
    for trigger in [
        "intake_received",
        "download_completed",
        "package_completed",
        "extraction_finished",
        "storage_threshold",
        "subscription_item",
    ] {
        ids.push(automation_for(&harness.router, trigger).await);
    }

    // The bus carries far more than automations should hang off. A configuration change or
    // a progress tick must not be a moment anything fires on.
    for (kind, payload) in [
        (EventKind::AccountChanged, json!({ "resource": "account" })),
        (
            EventKind::ProxyChanged,
            json!({ "resource": "proxy_profile" }),
        ),
        (EventKind::PluginChanged, json!({ "resource": "plugin" })),
        (EventKind::CaptchaChanged, json!({})),
        (EventKind::DownloadProgress, json!({ "download_id": "x" })),
        (
            EventKind::PostprocessProgress,
            json!({ "owner_id": "o", "kind": "extract_rar", "state": "running" }),
        ),
        (
            EventKind::SubscriptionChanged,
            json!({ "resource": "subscription" }),
        ),
    ] {
        harness
            .database
            .broadcast(EventEnvelope::new(kind, payload));
    }

    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let (_, runs) = get_json(&harness.router, "/api/v1/automations/runs").await;
    assert_eq!(
        runs.as_array().map(Vec::len),
        Some(0),
        "an event that is not a lifecycle moment started a run: {runs}"
    );
    let _ = ids;
}

fn common_download(package_id: rd_core::PackageId, file_name: &str) -> rd_db::NewDownload {
    rd_db::NewDownload {
        id: rd_core::DownloadId::new(),
        package_id,
        source: format!("https://example.com/{file_name}")
            .parse()
            .expect("url"),
        file_name: file_name.to_owned(),
        total_bytes: rd_core::ByteCount::new(1024).ok(),
        expected_checksum: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        initial_state: rd_core::DownloadState::Queued,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        enrichment: Vec::new(),
        secret_fragment: None,
    }
}
