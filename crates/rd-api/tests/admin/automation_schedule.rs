//! The time trigger and the actions of RD-1240-10 over REST: stored, refused, tried out,
//! exported and imported, and one action run end to end.

use crate::common;

use axum::{Router, http::StatusCode};
use common::{WAIT, eventually, get_json, parked_harness, post_json, test_harness, test_router};
use rd_core::{EventEnvelope, EventKind};
use serde_json::{Value, json};

async fn create(router: &Router, body: Value) -> String {
    let (status, created) = post_json(router, "/api/v1/automations", body).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("id").to_owned()
}

async fn target(router: &Router, name: &str) -> String {
    let (status, target) = post_json(
        router,
        "/api/v1/notifications/targets",
        json!({ "name": name, "kind": "webhook", "endpoint": "https://hooks.example.com/rd" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{target}");
    target["id"].as_str().expect("id").to_owned()
}

#[tokio::test]
async fn a_time_trigger_is_stored_with_its_schedule_and_refused_without_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (_, vocabulary) = get_json(&router, "/api/v1/automations/vocabulary").await;
    assert!(
        vocabulary["triggers"]
            .as_array()
            .expect("triggers")
            .contains(&json!("schedule"))
    );
    for kind in [
        "set_priority",
        "pause_queue",
        "start_queue",
        "extract_package",
        "notify",
        "add_links",
    ] {
        assert!(
            vocabulary["action_kinds"]
                .as_array()
                .expect("kinds")
                .contains(&json!(kind)),
            "{kind} missing from {vocabulary}"
        );
    }

    let schedule = json!({ "kind": "cron", "expression": "0 6 * * 1-5" });
    create(
        &router,
        json!({ "name": "Start at six", "enabled": true, "trigger": "schedule",
                "schedule": schedule, "actions": [{ "kind": "start_queue" }] }),
    )
    .await;
    // A schedule sent with another trigger is not stored: nothing would read it.
    create(
        &router,
        json!({ "name": "Plain", "trigger": "package_completed", "schedule": schedule,
                "actions": [{ "kind": "set_priority", "priority": "high" }] }),
    )
    .await;
    let (_, listed) = get_json(&router, "/api/v1/automations").await;
    let listed = listed.as_array().expect("list");
    let timed = listed
        .iter()
        .find(|item| item["name"] == "Start at six")
        .expect("timed");
    assert_eq!(timed["definition"]["schedule"], schedule);
    let plain = listed
        .iter()
        .find(|item| item["name"] == "Plain")
        .expect("plain");
    assert!(plain["definition"]["schedule"].is_null(), "{plain}");

    let cases = [
        (
            json!({ "name": "No schedule", "trigger": "schedule",
                    "actions": [{ "kind": "start_queue" }] }),
            "automation.schedule_invalid",
        ),
        (
            json!({ "name": "Never", "trigger": "schedule",
                    "schedule": { "kind": "cron", "expression": "0 0 30 2 *" },
                    "actions": [{ "kind": "start_queue" }] }),
            "automation.schedule_invalid",
        ),
        (
            json!({ "name": "No package", "trigger": "schedule",
                    "schedule": { "kind": "interval", "minutes": 60 },
                    "actions": [{ "kind": "extract_package" }] }),
            "automation.action_needs_package",
        ),
        (
            json!({ "name": "Loop", "trigger": "intake_received",
                    "actions": [{ "kind": "add_links", "links": ["https://example.com/a"] }] }),
            "automation.links_loop",
        ),
        (
            json!({ "name": "Magnet direct", "trigger": "package_completed",
                    "actions": [{ "kind": "add_links", "destination": "downloads",
                                  "links": ["magnet:?xt=urn:btih:abc"] }] }),
            "automation.links_invalid",
        ),
        (
            json!({ "name": "Silent", "trigger": "package_completed",
                    "actions": [{ "kind": "notify", "message": " ",
                                  "target_id": "00000000-0000-0000-0000-000000000001" }] }),
            "automation.notify_message_invalid",
        ),
    ];
    for (body, code) in cases {
        let (status, response) = post_json(&router, "/api/v1/automations", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(response["code"], code, "{response}");
    }
}

#[tokio::test]
async fn the_dry_run_names_the_next_run_and_what_would_run() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let target_id = target(&router, "Ops").await;
    let actions = json!([
        { "kind": "start_queue" },
        { "kind": "notify", "target_id": target_id, "message": "Night queue started" }
    ]);
    let (status, result) = post_json(
        &router,
        "/api/v1/automations/dry-run",
        json!({ "trigger": "schedule", "draft": {
            "trigger": "schedule", "condition": { "type": "always" },
            "schedule": { "kind": "interval", "minutes": 60 }, "actions": actions } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let entry = &result[0];
    assert_eq!(entry["trigger_matches"], true, "{result}");
    assert_eq!(entry["condition_matches"], true, "{result}");
    assert!(entry["next_run_at"].is_string(), "{result}");
    assert_eq!(entry["actions"], actions);
    // Another trigger has no next run.
    let (_, other) = post_json(
        &router,
        "/api/v1/automations/dry-run",
        json!({ "trigger": "package_completed", "draft": {
            "trigger": "package_completed", "actions": [{ "kind": "start_queue" }] } }),
    )
    .await;
    assert!(other[0]["next_run_at"].is_null(), "{other}");
}

#[tokio::test]
async fn export_and_import_carry_the_time_trigger_and_the_new_actions() {
    let source_dir = tempfile::tempdir().expect("tempdir");
    let source = test_router(source_dir.path()).await;
    let target_id = target(&source, "Ops").await;
    let schedule = json!({ "kind": "interval", "minutes": 30 });
    create(
        &source,
        json!({ "name": "Nightly", "enabled": true, "trigger": "schedule", "schedule": schedule,
                "actions": [
                    { "kind": "start_queue" },
                    { "kind": "notify", "target_id": target_id, "message": "Started" },
                    { "kind": "add_links", "destination": "downloads",
                      "links": ["https://example.com/file.zip"] }
                ] }),
    )
    .await;
    create(
        &source,
        json!({ "name": "Finish", "trigger": "package_completed", "actions": [
            { "kind": "set_priority", "priority": "low" }, { "kind": "extract_package" } ] }),
    )
    .await;

    let (status, bundle) = get_json(&source, "/api/v1/automations/export").await;
    assert_eq!(status, StatusCode::OK, "{bundle}");
    let entries = bundle["automations"].as_array().expect("entries");
    let nightly = entries
        .iter()
        .find(|entry| entry["name"] == "Nightly")
        .expect("nightly");
    assert_eq!(nightly["schedule"], schedule);
    // The target travels by name, as a webhook's does.
    assert_eq!(nightly["actions"][1]["target_name"], "Ops");
    assert!(nightly["actions"][1].get("target_id").is_none());

    let copy_dir = tempfile::tempdir().expect("tempdir");
    let copy = test_router(copy_dir.path()).await;
    target(&copy, "Ops").await;
    let (status, summary) = post_json(&copy, "/api/v1/automations/import", bundle).await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["created"], 2, "{summary}");
    let (_, listed) = get_json(&copy, "/api/v1/automations").await;
    let listed = listed.as_array().expect("list");
    let nightly = listed
        .iter()
        .find(|item| item["name"] == "Nightly")
        .expect("nightly");
    assert_eq!(nightly["definition"]["schedule"], schedule);
    let kinds: Vec<&str> = nightly["definition"]["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .filter_map(|action| action["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["start_queue", "notify", "add_links"]);
    let finish = listed
        .iter()
        .find(|item| item["name"] == "Finish")
        .expect("finish");
    assert_eq!(finish["definition"]["actions"][0]["priority"], "low");
    assert_eq!(
        finish["definition"]["actions"][1]["kind"],
        "extract_package"
    );
}

#[tokio::test]
async fn add_links_hands_its_links_to_the_linkgrabber() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let link = "https://files.example.invalid/automation/release.bin";
    let automation = create(
        &harness.router,
        json!({ "name": "Fetch on full disk", "enabled": true, "trigger": "storage_threshold",
                "actions": [{ "kind": "add_links", "links": [link] }] }),
    )
    .await;
    harness.database.broadcast(EventEnvelope::new(
        EventKind::StorageCapacity,
        json!({ "target": "root", "blocked": true }),
    ));
    let router = &harness.router;
    let automation = &automation;
    eventually(WAIT, "the add_links run did not complete", || async move {
        let (_, runs) = get_json(
            router,
            &format!("/api/v1/automations/runs?automation_id={automation}"),
        )
        .await;
        (runs[0]["state"] == "completed").then_some(())
    })
    .await;
    let (_, candidates) = get_json(router, "/api/v1/collector/candidates").await;
    assert!(
        candidates
            .as_array()
            .expect("candidates")
            .iter()
            .any(|candidate| candidate["url"] == link),
        "{candidates}"
    );
}

/// An automation runs unattended, so its links keep to the rule of a proposed link from the
/// person's own intake (the hot folder's `.rdlinks`): a link to this machine is marked and never
/// requested.
#[tokio::test]
async fn an_automation_s_link_to_this_machine_is_held_like_a_proposed_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let link = "http://127.0.0.1:9/automation/held.bin";
    create(
        &harness.router,
        json!({ "name": "Fetch local", "enabled": true, "trigger": "storage_threshold",
                "actions": [{ "kind": "add_links", "links": [link] }] }),
    )
    .await;
    harness.database.broadcast(EventEnvelope::new(
        EventKind::StorageCapacity,
        json!({ "target": "root", "blocked": true }),
    ));
    let database = &harness.database;
    let candidate = eventually(
        WAIT,
        "the automation's link was not checked",
        || async move {
            database
                .list_candidates()
                .await
                .ok()?
                .into_iter()
                .find(|candidate| {
                    candidate.url.as_str() == link
                        && candidate.checked_at.is_some()
                        && candidate.state != rd_core::LinkCandidateState::Checking
                })
        },
    )
    .await;
    assert_eq!(
        candidate.error_code.as_deref(),
        Some("collector.check_internal_address"),
        "{candidate:?}"
    );
    let reaches = database.candidates_remote_reach().await.expect("reaches");
    assert_eq!(reaches.get(&candidate.id), Some(&true), "{reaches:?}");
}
