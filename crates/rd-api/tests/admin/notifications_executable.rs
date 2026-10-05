//! RD-1101-11 (audit 2026-10-05, S1): the program an apprise target starts is named by an
//! administrator only, like the program paths among the settings, and a path no administrator
//! saved is never started — not by a delivery, not by the test action.

use axum::http::StatusCode;
use rd_notify::EXECUTABLE_SEAL;
use serde_json::json;

use crate::common;

/// Holds configuration and nothing else: it edits targets, it does not name programs.
const CONFIG_BEARER: &str = "test-config-bearer-token";
const TARGETS: &str = "/api/v1/notifications/targets";

async fn harness(directory: &std::path::Path) -> common::Harness {
    common::harness(
        directory,
        common::Options::default()
            .login()
            .token(CONFIG_BEARER, rd_core::API_CONFIG_SCOPE),
    )
    .await
}

/// An apprise target naming `executable`, as the API takes it.
fn apprise(executable: &str) -> serde_json::Value {
    json!({
        "name": "Phone",
        "kind": "apprise",
        "endpoint": "tgram",
        "config": { "executable": executable },
        "secret": "tgram://token/chat"
    })
}

fn assert_needs_admin(status: StatusCode, body: &serde_json::Value) {
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "auth.scope_insufficient", "{body}");
    assert_eq!(body["params"]["scope"], "api:admin", "{body}");
    assert_eq!(body["params"]["setting"], "executable", "{body}");
}

#[tokio::test]
async fn a_configuration_token_cannot_name_the_program_a_target_runs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let router = &harness.router;

    let (status, body) =
        common::post_with_bearer(router, TARGETS, CONFIG_BEARER, apprise("/bin/sh")).await;
    assert_needs_admin(status, &body);
    assert!(
        harness
            .database
            .list_notification_targets()
            .await
            .expect("targets")
            .is_empty(),
        "the refused target was stored"
    );

    // The gate is the path, not the kind: without one the same token creates the target ...
    let mut plain = apprise("");
    plain["config"] = json!({});
    let (status, target) = common::post_with_bearer(router, TARGETS, CONFIG_BEARER, plain).await;
    assert_eq!(status, StatusCode::CREATED, "{target}");
    // ... and cannot add one afterwards.
    let uri = format!("{TARGETS}/{}", target["id"].as_str().expect("id"));
    let (status, body) =
        common::put_with_bearer(router, &uri, CONFIG_BEARER, apprise("/bin/sh")).await;
    assert_needs_admin(status, &body);
}

#[tokio::test]
async fn an_administrator_names_it_and_a_configuration_token_keeps_it_unchanged() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let router = &harness.router;
    let path = "/opt/apprise/bin/apprise";

    let (status, target) =
        common::post_with_bearer(router, TARGETS, common::API_BEARER, apprise(path)).await;
    assert_eq!(status, StatusCode::CREATED, "{target}");
    assert_eq!(target["config"]["executable"], path, "{target}");
    assert!(target["config"][EXECUTABLE_SEAL].is_string(), "{target}");
    let uri = format!("{TARGETS}/{}", target["id"].as_str().expect("id"));

    // A rename sends the whole configuration back, as the interface does; the path and its
    // seal stay, and the stored apprise URL with them.
    let renamed = json!({
        "name": "Renamed",
        "kind": "apprise",
        "endpoint": "tgram",
        "config": target["config"].clone()
    });
    let (status, saved) = common::put_with_bearer(router, &uri, CONFIG_BEARER, renamed).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["has_secret"], true, "{saved}");
    assert_eq!(
        saved["config"][EXECUTABLE_SEAL], target["config"][EXECUTABLE_SEAL],
        "{saved}"
    );

    let (status, body) =
        common::put_with_bearer(router, &uri, CONFIG_BEARER, apprise("/bin/sh")).await;
    assert_needs_admin(status, &body);
}

/// A row as a configuration token could store it before RD-1101-11 — the path verbatim, and a
/// seal of its own making beside it — is never started by the test action. A configuration
/// token's save does not change that; an administrator's does.
#[cfg(unix)]
#[tokio::test]
async fn the_test_action_starts_only_a_path_an_administrator_saved() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let router = &harness.router;
    let ran = directory.path().join("ran");
    let script = directory.path().join("planted");
    std::fs::write(&script, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).expect("script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = script.to_string_lossy().into_owned();

    let reference = harness
        .secrets
        .put_string("tgram://token/chat".to_owned())
        .await
        .expect("vault");
    let planted = harness
        .database
        .upsert_notification_target(
            None,
            rd_db::NewNotificationTarget {
                name: "planted".to_owned(),
                kind: rd_notify::TargetKind::Apprise,
                enabled: true,
                endpoint: "tgram".to_owned(),
                config: json!({ "executable": path, EXECUTABLE_SEAL: "00".repeat(32) }),
                secret_ref: Some(reference),
                clear_secret: false,
            },
        )
        .await
        .expect("planted target");
    let uri = format!("{TARGETS}/{}", planted.id);
    let test = format!("{uri}/test");
    let resave = json!({
        "name": "planted",
        "kind": "apprise",
        "endpoint": "tgram",
        "config": planted.config
    });

    let (status, answer) = common::request_with_bearer(router, "POST", &test, CONFIG_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        !ran.exists(),
        "the test action started a path nobody approved"
    );

    let (status, saved) =
        common::put_with_bearer(router, &uri, CONFIG_BEARER, resave.clone()).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (_, answer) = common::request_with_bearer(router, "POST", &test, CONFIG_BEARER).await;
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(
        !ran.exists(),
        "a configuration token's save approved the path"
    );

    let (status, saved) = common::put_with_bearer(router, &uri, common::API_BEARER, resave).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (_, answer) = common::request_with_bearer(router, "POST", &test, CONFIG_BEARER).await;
    assert_eq!(answer["ok"], true, "{answer}");
    assert!(ran.exists(), "the administrator's path was not started");
}
