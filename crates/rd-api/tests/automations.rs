//! The automation engine end to end: definitions, validation, dry runs and real triggers.

mod common;

use axum::http::StatusCode;
use common::{delete_json, get_json, parked_harness, post_json, test_harness};
use serde_json::json;

fn definition(name: &str, enabled: bool) -> serde_json::Value {
    json!({
        "name": name,
        "enabled": enabled,
        "trigger": "download_completed",
        "condition": { "type": "always" },
        "actions": [{ "kind": "pause_package" }]
    })
}

#[tokio::test]
async fn an_automation_is_created_listed_and_versioned() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, created) = post_json(
        &harness.router,
        "/api/v1/automations",
        definition("Pause on completion", false),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    assert_eq!(created["version"], 1, "{created}");

    let (_, listed) = get_json(&harness.router, "/api/v1/automations").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    assert_eq!(
        listed[0]["definition"]["trigger"], "download_completed",
        "the list carries the definition in force: {listed}"
    );

    // Editing writes a new version rather than rewriting the one runs point at.
    let request = axum::http::Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/automations/{id}"))
        .header(axum::http::header::HOST, "127.0.0.1:8710")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            definition("Pause on completion", true).to_string(),
        ))
        .expect("request");
    let response = tower::ServiceExt::oneshot(harness.router.clone(), request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let (_, versions) = get_json(
        &harness.router,
        &format!("/api/v1/automations/{id}/versions"),
    )
    .await;
    assert_eq!(versions.as_array().map(Vec::len), Some(2), "{versions}");
}

#[tokio::test]
async fn an_unevaluatable_definition_is_refused_with_its_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    // Each of these would otherwise become a rule that silently never fires, and the only
    // way to find out would be to wonder why nothing happened.
    let cases = [
        (
            json!({ "name": "", "trigger": "download_completed", "condition": {"type":"always"},
                    "actions": [{"kind":"pause_package"}] }),
            "automation.name_invalid",
        ),
        (
            json!({ "name": "No actions", "trigger": "download_completed",
                    "condition": {"type":"always"}, "actions": [] }),
            "automation.action_count_invalid",
        ),
        (
            json!({ "name": "Bad regex", "trigger": "download_completed",
                    "condition": {"type":"predicate","predicate":{"field":"name",
                        "operator":"matches","value":"S0[12"}},
                    "actions": [{"kind":"pause_package"}] }),
            "automation.predicate_invalid",
        ),
        (
            json!({ "name": "Wrong operator", "trigger": "download_completed",
                    "condition": {"type":"predicate","predicate":{"field":"size_bytes",
                        "operator":"contains","value":"10"}},
                    "actions": [{"kind":"pause_package"}] }),
            "automation.predicate_invalid",
        ),
        (
            json!({ "name": "Escaping script", "trigger": "download_completed",
                    "condition": {"type":"always"},
                    "actions": [{"kind":"script","name":"../../etc/passwd"}] }),
            "automation.script_name_invalid",
        ),
        (
            json!({ "name": "Empty group", "trigger": "download_completed",
                    "condition": {"type":"all","nodes":[]},
                    "actions": [{"kind":"pause_package"}] }),
            "automation.predicate_invalid",
        ),
    ];
    for (body, code) in cases {
        let (status, response) = post_json(&harness.router, "/api/v1/automations", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{response}");
        assert_eq!(response["code"], code, "{response}");
    }
    let (_, listed) = get_json(&harness.router, "/api/v1/automations").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(0), "{listed}");
}

#[tokio::test]
async fn a_dry_run_reports_matches_without_running_anything() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (_, created) = post_json(
        &harness.router,
        "/api/v1/automations",
        json!({
            "name": "Only mkv",
            "enabled": true,
            "trigger": "download_completed",
            "condition": { "type": "predicate", "predicate": {
                "field": "name", "operator": "ends_with", "value": ".mkv" } },
            "actions": [{ "kind": "pause_package" }]
        }),
    )
    .await;
    let automation_id = created["id"].as_str().expect("id").to_owned();

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Sample.Release.mkv".to_owned(),
            destination: directory.path().join("out").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    harness
        .database
        .create_download(download_for(package.id, "Sample.Release.mkv"))
        .await
        .expect("download");

    let (status, matches) = post_json(
        &harness.router,
        "/api/v1/automations/dry-run",
        json!({ "trigger": "download_completed", "package_id": package.id }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{matches}");
    assert_eq!(matches[0]["automation_id"], automation_id, "{matches}");
    assert_eq!(matches[0]["trigger_matches"], true, "{matches}");
    assert_eq!(matches[0]["condition_matches"], true, "{matches}");

    // The point of a dry run: nothing was queued and nothing was executed.
    let (_, runs) = get_json(&harness.router, "/api/v1/automations/runs").await;
    assert_eq!(runs.as_array().map(Vec::len), Some(0), "{runs}");
}

#[tokio::test]
async fn a_dry_run_shows_a_condition_that_does_not_hold() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    post_json(
        &harness.router,
        "/api/v1/automations",
        json!({
            "name": "Only mp4",
            "enabled": true,
            "trigger": "download_completed",
            "condition": { "type": "predicate", "predicate": {
                "field": "name", "operator": "ends_with", "value": ".mp4" } },
            "actions": [{ "kind": "pause_package" }]
        }),
    )
    .await;

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Sample.Release.mkv".to_owned(),
            destination: directory.path().join("out").to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");

    let (_, matches) = post_json(
        &harness.router,
        "/api/v1/automations/dry-run",
        json!({ "trigger": "package_completed", "package_id": package.id }),
    )
    .await;
    // Trigger and condition are reported apart, so the author can tell "wrong moment" from
    // "right moment, rule does not hold".
    assert_eq!(matches[0]["trigger_matches"], false, "{matches}");
    assert_eq!(matches[0]["condition_matches"], false, "{matches}");
}

#[tokio::test]
async fn a_completed_download_queues_a_run_for_a_matching_automation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;

    post_json(
        &harness.router,
        "/api/v1/automations",
        json!({
            "name": "Pause finished mkv",
            "enabled": true,
            "trigger": "download_completed",
            "condition": { "type": "predicate", "predicate": {
                "field": "extension", "operator": "equals", "value": "mkv" } },
            "actions": [{ "kind": "pause_package" }]
        }),
    )
    .await;

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Triggering.Release".to_owned(),
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
        .create_download(download_for(package.id, "Triggering.Release.mkv"))
        .await
        .expect("download");

    // The real path a job walks: queued, resolving, downloading, verifying, completed.
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

    let runs = wait_for_runs(&harness.router, 1).await;
    assert!(runs[0]["automation_id"].as_str().is_some(), "{runs:?}");
    assert_eq!(
        runs[0]["package_id"].as_str(),
        Some(package.id.to_string().as_str()),
        "the run carries the package its actions operate on: {runs:?}"
    );
}

#[tokio::test]
async fn a_disabled_automation_starts_no_runs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;

    let (_, created) = post_json(
        &harness.router,
        "/api/v1/automations",
        definition("Disabled", true),
    )
    .await;
    let id = created["id"].as_str().expect("id").to_owned();
    let (status, _) = post_json(
        &harness.router,
        &format!("/api/v1/automations/{id}/enable"),
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let package = harness
        .database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "Ignored.Release".to_owned(),
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
        .create_download(download_for(package.id, "Ignored.Release.mkv"))
        .await
        .expect("download");
    // The real path a job walks: queued, resolving, downloading, verifying, completed.
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

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (_, runs) = get_json(&harness.router, "/api/v1/automations/runs").await;
    assert_eq!(runs.as_array().map(Vec::len), Some(0), "{runs}");
}

#[tokio::test]
async fn deleting_an_automation_removes_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (_, created) = post_json(
        &harness.router,
        "/api/v1/automations",
        definition("Temporary", false),
    )
    .await;
    let id = created["id"].as_str().expect("id").to_owned();
    let (status, _) = delete_json(&harness.router, &format!("/api/v1/automations/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    let (_, listed) = get_json(&harness.router, "/api/v1/automations").await;
    assert_eq!(listed.as_array().map(Vec::len), Some(0), "{listed}");
}

#[tokio::test]
async fn the_editor_vocabulary_matches_what_the_engine_accepts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, vocabulary) = get_json(&harness.router, "/api/v1/automations/vocabulary").await;
    assert_eq!(status, StatusCode::OK);
    // Every trigger the vocabulary offers has to be one the API accepts, or the editor can
    // build a definition the server refuses.
    for trigger in vocabulary["triggers"].as_array().expect("triggers") {
        let body = json!({
            "name": "Vocabulary probe",
            "trigger": trigger,
            "condition": { "type": "always" },
            "actions": [{ "kind": "pause_package" }]
        });
        let (status, response) = post_json(&harness.router, "/api/v1/automations", body).await;
        assert_eq!(status, StatusCode::CREATED, "{trigger}: {response}");
    }
    assert!(vocabulary["max_actions"].as_u64().unwrap_or(0) > 0);
}

/// Polls the run history until it holds `expected` entries.
async fn wait_for_runs(router: &axum::Router, expected: usize) -> Vec<serde_json::Value> {
    for _ in 0..40 {
        let (_, runs) = get_json(router, "/api/v1/automations/runs").await;
        if let Some(items) = runs.as_array()
            && items.len() >= expected
        {
            return items.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no automation run appeared within two seconds");
}

fn download_for(package_id: rd_core::PackageId, file_name: &str) -> rd_db::NewDownload {
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

/// An action that names another record round-trips by id.
///
/// Both id-bearing actions went unexercised, and the editor sent an empty string for the id it
/// had no field for. That reached serde before any of the `automation.*` codes, so the interface
/// showed a raw parser message. The contract is checked here from both sides: a real id is kept,
/// and an empty one is refused.
#[tokio::test]
async fn an_action_keeps_the_target_it_names() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, target) = post_json(
        &harness.router,
        "/api/v1/notifications/targets",
        json!({
            "name": "Ops webhook",
            "kind": "webhook",
            "endpoint": "https://hooks.example.com/rd"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{target}");
    let target_id = target["id"].as_str().expect("target id").to_owned();

    let (status, created) = post_json(
        &harness.router,
        "/api/v1/automations",
        json!({
            "name": "Announce completed packages",
            "enabled": true,
            "trigger": "package_completed",
            "condition": { "type": "always" },
            "actions": [{ "kind": "webhook", "target_id": target_id }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let (_, listed) = get_json(&harness.router, "/api/v1/automations").await;
    assert_eq!(
        listed[0]["definition"]["actions"][0]["target_id"], target_id,
        "the action still names the target it was given: {listed}"
    );
}

#[tokio::test]
async fn an_action_without_an_id_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let (status, refused) = post_json(
        &harness.router,
        "/api/v1/automations",
        json!({
            "name": "Announce completed packages",
            "enabled": true,
            "trigger": "package_completed",
            "condition": { "type": "always" },
            "actions": [{ "kind": "webhook", "target_id": "" }]
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an empty id is not a UUID: {refused}"
    );
}
