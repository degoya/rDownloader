//! Resetting an automation's version history over REST (RD-1280-01): only the version in force
//! stays, as version 1; refused while a run of an older version is still active.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, put_json, test_harness};
use rd_automation::RunState;
use serde_json::json;

fn definition(pause: bool) -> serde_json::Value {
    let action = if pause {
        "pause_package"
    } else {
        "resume_package"
    };
    json!({
        "name": "Tidy up",
        "enabled": true,
        "trigger": "download_completed",
        "condition": { "type": "always" },
        "actions": [{ "kind": action }]
    })
}

#[tokio::test]
async fn a_reset_keeps_the_version_in_force_and_waits_for_an_older_run() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    // The engine would otherwise sweep the run this test parks in a state of its choosing.
    harness.state.automations.shutdown();

    let (status, created) =
        post_json(&harness.router, "/api/v1/automations", definition(true)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    for pause in [false, true] {
        let (status, saved) = put_json(
            &harness.router,
            &format!("/api/v1/automations/{id}"),
            definition(pause),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
    }
    let automation_id: rd_core::AutomationId = id.parse().expect("automation id");
    let versions = harness
        .database
        .automation_versions(automation_id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 3);
    let (current, first) = (versions[0].clone(), versions[2].clone());

    // A run of version 1 still waiting for its retry.
    let event = rd_core::EventId::new();
    harness
        .database
        .queue_automation_run(rd_db::NewRun {
            automation_id,
            automation_version_id: first.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(first.id, &event),
        })
        .await
        .expect("queue");
    let run = harness
        .database
        .automation_runs(Some(automation_id), 10)
        .await
        .expect("runs")[0]
        .id;
    let later = chrono::Utc::now() + chrono::Duration::hours(1);
    harness
        .database
        .record_automation_attempt(run, RunState::Retrying, 0, 1, Some(later), None)
        .await
        .expect("retrying");

    let reset = format!("/api/v1/automations/{id}/versions/reset");
    let (status, refused) = post_json(&harness.router, &reset, json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(
        refused["code"], "automation.version_reset_run_active",
        "{refused}"
    );
    let (_, listed) = get_json(
        &harness.router,
        &format!("/api/v1/automations/{id}/versions"),
    )
    .await;
    assert_eq!(
        listed.as_array().map(Vec::len),
        Some(3),
        "nothing went: {listed}"
    );

    harness
        .database
        .record_automation_attempt(run, RunState::Failed, 0, 2, None, None)
        .await
        .expect("given up");
    let (status, answer) = post_json(&harness.router, &reset, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["removed_versions"], 2, "{answer}");
    assert_eq!(answer["removed_runs"], 1, "{answer}");
    assert_eq!(answer["automation"]["version"], 1, "{answer}");

    let (_, listed) = get_json(
        &harness.router,
        &format!("/api/v1/automations/{id}/versions"),
    )
    .await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(listed[0]["id"], current.id.to_string(), "{listed}");
    assert_eq!(listed[0]["version"], 1, "{listed}");
    let (_, automations) = get_json(&harness.router, "/api/v1/automations").await;
    assert_eq!(automations[0]["version"], 1, "{automations}");
    assert_eq!(
        automations[0]["definition"]["id"],
        current.id.to_string(),
        "the definition in force is the one kept: {automations}"
    );
    let (_, runs) = get_json(
        &harness.router,
        &format!("/api/v1/automations/runs?automation_id={id}"),
    )
    .await;
    assert_eq!(runs.as_array().map(Vec::len), Some(0), "{runs}");
}

#[tokio::test]
async fn resetting_an_unknown_automation_is_not_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, answer) = post_json(
        &harness.router,
        &format!(
            "/api/v1/automations/{}/versions/reset",
            rd_core::AutomationId::new()
        ),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "automation.not_found", "{answer}");
}
