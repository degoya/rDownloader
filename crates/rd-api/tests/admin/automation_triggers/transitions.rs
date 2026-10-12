//! The package and storage triggers decided on the event's payload, end to end (RD-1270-03,
//! RD-1270-04), the storage target among it (RD-1270-18).
//!
//! A package trigger used to read the package row by the time the event was read: every reorder
//! of a finished package was one more "package finished", and an outcome read after a restart was
//! lost. A storage rule ran on a root that had room again exactly as on one that ran out. Each
//! test brackets its events between two intakes a witness automation runs on, so a count is taken
//! only once the engine has read past the last event.

use super::{automation_for, common_download, intake, run_count, wait_for_runs_of};
use crate::common;

use axum::http::StatusCode;
use common::{get_json, parked_harness, patch_json, post_json, test_harness};
use rd_core::{EventEnvelope, EventKind, PackageId, PackageState, PostprocessStage};
use serde_json::json;

/// Creates a package without files in `directory`.
async fn package(
    harness: &common::Harness,
    directory: &std::path::Path,
    name: &str,
) -> rd_core::DownloadPackage {
    harness
        .database
        .create_package(rd_db::NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: directory.join("out").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package")
}

async fn set_state(harness: &common::Harness, id: PackageId, state: PackageState) {
    harness
        .database
        .set_package_state(id, state, None, None, None)
        .await
        .expect("package state");
}

/// Sends an intake past everything before it and waits until the witness has run `count` times.
async fn read_past(harness: &common::Harness, witness: &str, count: usize) {
    harness.database.broadcast(intake("after"));
    wait_for_runs_of(
        &harness.router,
        witness,
        count,
        "the engine never read past the events",
    )
    .await;
}

#[tokio::test]
async fn a_finished_package_fires_once_whatever_happens_to_it_afterwards() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let witness = automation_for(&harness.router, "intake_received").await;
    let completed = automation_for(&harness.router, "package_completed").await;

    let finished = package(&harness, directory.path(), "Finished.Release").await;
    let mut files = Vec::new();
    for name in ["Finished.Release.part1.rar", "Finished.Release.part2.rar"] {
        let file = harness
            .database
            .create_download(common_download(finished.id, name))
            .await
            .expect("download");
        files.push(file.id);
    }
    // Its stages, which the engine may well read after the outcome below was written.
    for stage in [PostprocessStage::Repairing, PostprocessStage::Extracting] {
        harness
            .database
            .set_package_state(
                finished.id,
                PackageState::Postprocessing,
                Some(stage),
                None,
                None,
            )
            .await
            .expect("stage");
    }
    // The outcome, the same outcome written once more, then reordered and renamed.
    set_state(&harness, finished.id, PackageState::Completed).await;
    set_state(&harness, finished.id, PackageState::Completed).await;
    files.reverse();
    harness
        .database
        .reorder_downloads(finished.id, files)
        .await
        .expect("reordered");
    let (status, renamed) = patch_json(
        &harness.router,
        &format!("/api/v1/packages/{}", finished.id),
        json!({ "name": "Renamed.Release" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");

    read_past(&harness, &witness, 1).await;
    assert_eq!(
        run_count(&harness.router, &completed).await,
        1,
        "one finished package is one run"
    );
}

#[tokio::test]
async fn an_outcome_counts_after_its_package_was_restarted_or_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let witness = automation_for(&harness.router, "intake_received").await;
    let completed = automation_for(&harness.router, "package_completed").await;
    let failed = automation_for(&harness.router, "package_failed").await;

    // Finished, then started again -- possibly before the engine read the outcome.
    let restarted = package(&harness, directory.path(), "Restarted.Release").await;
    set_state(&harness, restarted.id, PackageState::Completed).await;
    set_state(&harness, restarted.id, PackageState::Queued).await;
    // Failed, then removed.
    let removed = package(&harness, directory.path(), "Removed.Release").await;
    set_state(&harness, removed.id, PackageState::Failed).await;
    assert!(
        harness
            .database
            .delete_empty_package(removed.id)
            .await
            .expect("removed"),
        "the empty package was not removed"
    );
    // Removed before its outcome was read, for certain: the outcome of a package with no row.
    let gone = PackageId::new();
    harness.database.broadcast(EventEnvelope::new(
        EventKind::PackageState,
        json!({
            "package_id": gone, "state": "completed", "previous": "postprocessing",
            "files": 1, "failed_files": 0
        }),
    ));

    read_past(&harness, &witness, 1).await;
    assert_eq!(run_count(&harness.router, &completed).await, 2);
    assert_eq!(run_count(&harness.router, &failed).await, 1);
    // Each run names the package its event named, removed or not.
    let (_, runs) = get_json(
        &harness.router,
        &format!("/api/v1/automations/runs?automation_id={completed}"),
    )
    .await;
    let named: Vec<&str> = runs
        .as_array()
        .expect("runs")
        .iter()
        .filter_map(|run| run["package_id"].as_str())
        .collect();
    for id in [restarted.id, gone] {
        assert!(named.contains(&id.to_string().as_str()), "{id}: {runs}");
    }
}

/// A storage rule named `name` on the trigger with the given condition.
async fn storage_rule(router: &axum::Router, name: &str, condition: serde_json::Value) -> String {
    let (status, created) = post_json(
        router,
        "/api/v1/automations",
        json!({
            "name": name,
            "enabled": true,
            "trigger": "storage_threshold",
            "condition": condition,
            "actions": [{ "kind": "pause_queue" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("id").to_owned()
}

#[tokio::test]
async fn a_storage_release_runs_only_the_rule_that_asks_for_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let witness = automation_for(&harness.router, "intake_received").await;
    // Written as every storage rule was before the change was a field: it means the block.
    let blocking = storage_rule(&harness.router, "Storage low", json!({ "type": "always" })).await;
    let releasing = storage_rule(
        &harness.router,
        "Storage free again",
        json!({
            "type": "predicate",
            "predicate": { "field": "change", "operator": "equals", "value": "released" }
        }),
    )
    .await;

    // The change of what `persist_capacity_blocks` (rd-scheduler) broadcasts: one block, two
    // releases.
    for change in ["blocked", "released", "released"] {
        harness.database.broadcast(EventEnvelope::new(
            EventKind::StorageCapacity,
            json!({ "blocked": u8::from(change == "blocked"), "change": change }),
        ));
    }

    read_past(&harness, &witness, 1).await;
    assert_eq!(run_count(&harness.router, &blocking).await, 1, "the block");
    assert_eq!(
        run_count(&harness.router, &releasing).await,
        2,
        "the releases"
    );
}

/// What `persist_capacity_blocks` (rd-scheduler) broadcasts for one target (RD-1270-18).
fn storage_event(change: &str, target: &str, name: &str, blocked: u8) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::StorageCapacity,
        json!({
            "blocked": blocked, "change": change, "target": target,
            "name": name, "path": format!("/srv/{}", name.to_lowercase())
        }),
    )
}

fn predicate(field: &str, value: &str) -> serde_json::Value {
    json!({
        "type": "predicate",
        "predicate": { "field": field, "operator": "equals", "value": value }
    })
}

/// The condition holds the target's key, not its name (owner, 2026-10-12): a root that is named
/// `Downloads` is not the download directory, whose key is `fallback`.
#[tokio::test]
async fn a_storage_rule_on_one_target_runs_for_that_target_only() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let witness = automation_for(&harness.router, "intake_received").await;
    let any_target = storage_rule(
        &harness.router,
        "Any storage low",
        json!({ "type": "always" }),
    )
    .await;
    let media = rd_core::StorageRootId::new().to_string();
    // A root that carries the download directory's name.
    let named_downloads = rd_core::StorageRootId::new().to_string();
    // Names the target, not the change: the block of that one root.
    let media_low = storage_rule(
        &harness.router,
        "Media low",
        predicate("storage_target", &media),
    )
    .await;
    let downloads_free = storage_rule(
        &harness.router,
        "Downloads free again",
        json!({
            "type": "all",
            "nodes": [
                predicate("change", "released"),
                predicate("storage_target", "fallback")
            ]
        }),
    )
    .await;

    for event in [
        storage_event("blocked", &media, "Media", 1),
        storage_event("blocked", "fallback", "Downloads", 2),
        storage_event("blocked", &named_downloads, "Downloads", 3),
        storage_event("released", &media, "Media", 2),
        storage_event("released", &named_downloads, "Downloads", 1),
        storage_event("released", "fallback", "Downloads", 0),
    ] {
        harness.database.broadcast(event);
    }

    read_past(&harness, &witness, 1).await;
    assert_eq!(
        run_count(&harness.router, &any_target).await,
        3,
        "every block"
    );
    assert_eq!(
        run_count(&harness.router, &media_low).await,
        1,
        "the media root's block"
    );
    assert_eq!(
        run_count(&harness.router, &downloads_free).await,
        1,
        "the download directory's release, not the root named like it"
    );
}
