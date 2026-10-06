//! The editor's dry run judges the draft it holds (RD-1120-17).
//!
//! The dry run used to judge only the saved, enabled automations, so a new automation, or one
//! switched off, answered "No enabled automation to evaluate" in the very editor that offered
//! the button. With a draft in the request only the draft is judged, saved or not, on or off.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_harness};
use serde_json::json;

const NIL_ID: &str = "00000000-0000-0000-0000-000000000000";

fn ends_with(suffix: &str) -> serde_json::Value {
    json!({ "type": "predicate", "predicate": {
        "field": "name", "operator": "ends_with", "value": suffix } })
}

async fn sample_package(harness: &common::Harness, root: &std::path::Path) -> rd_core::PackageId {
    harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Draft.Sample.mkv".to_owned(),
            destination: root.join("out").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package")
        .id
}

#[tokio::test]
async fn a_draft_that_is_neither_saved_nor_enabled_is_judged() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let package_id = sample_package(&harness, directory.path()).await;

    // Without a draft there is nothing to judge: no automation is saved.
    let (status, matches) = post_json(
        &harness.router,
        "/api/v1/automations/dry-run",
        json!({ "trigger": "download_completed", "package_id": package_id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{matches}");
    assert_eq!(matches.as_array().map(Vec::len), Some(0), "{matches}");

    let (status, matches) = post_json(
        &harness.router,
        "/api/v1/automations/dry-run",
        json!({
            "trigger": "download_completed",
            "package_id": package_id,
            "draft": { "trigger": "download_completed", "condition": ends_with(".mkv") }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{matches}");
    assert_eq!(matches.as_array().map(Vec::len), Some(1), "{matches}");
    assert_eq!(matches[0]["automation_id"], NIL_ID, "{matches}");
    assert_eq!(matches[0]["trigger_matches"], true, "{matches}");
    assert_eq!(matches[0]["condition_matches"], true, "{matches}");

    // Judging is all it does: nothing is stored and nothing is queued.
    let (_, automations) = get_json(&harness.router, "/api/v1/automations").await;
    assert_eq!(
        automations.as_array().map(Vec::len),
        Some(0),
        "{automations}"
    );
    let (_, runs) = get_json(&harness.router, "/api/v1/automations/runs").await;
    assert_eq!(runs.as_array().map(Vec::len), Some(0), "{runs}");
}

#[tokio::test]
async fn the_draft_of_a_saved_automation_is_judged_instead_of_what_is_stored() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let package_id = sample_package(&harness, directory.path()).await;
    let (_, edited) = post_json(
        &harness.router,
        "/api/v1/automations",
        json!({
            "name": "Switched off",
            "enabled": false,
            "trigger": "package_completed",
            "condition": ends_with(".mkv"),
            "actions": [{ "kind": "pause_package" }]
        }),
    )
    .await;
    let edited_id = edited["id"].as_str().expect("id").to_owned();
    post_json(
        &harness.router,
        "/api/v1/automations",
        json!({
            "name": "Another, enabled",
            "enabled": true,
            "trigger": "download_completed",
            "condition": { "type": "always" },
            "actions": [{ "kind": "pause_package" }]
        }),
    )
    .await;

    // The editor changed trigger and condition and has not saved: the draft is what counts,
    // and the enabled automation beside it is not part of the answer.
    let (status, matches) = post_json(
        &harness.router,
        "/api/v1/automations/dry-run",
        json!({
            "trigger": "download_completed",
            "package_id": package_id,
            "draft": {
                "automation_id": edited_id,
                "trigger": "download_completed",
                "condition": ends_with(".mp4")
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{matches}");
    assert_eq!(matches.as_array().map(Vec::len), Some(1), "{matches}");
    assert_eq!(matches[0]["automation_id"], edited_id, "{matches}");
    assert_eq!(matches[0]["trigger_matches"], true, "{matches}");
    assert_eq!(matches[0]["condition_matches"], false, "{matches}");
}

#[tokio::test]
async fn a_draft_whose_condition_cannot_be_evaluated_is_refused_like_a_save() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/automations/dry-run",
        json!({
            "trigger": "download_completed",
            "draft": { "trigger": "download_completed", "condition": { "type": "all", "nodes": [] } }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "automation.predicate_invalid", "{body}");
}
