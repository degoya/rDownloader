//! Removing every superseded version at once (RD-1140-04).
//!
//! The owner's request: a plugin updated a few times carries one leftover version per update, and
//! removing them one confirmation at a time was the chore. What these hold: every superseded
//! version of every plugin, or of one, goes, audited; the version that runs never does; the one
//! the next start loads and the one under test stay and are named; a version unfinished work is
//! bound to stays and is named with the refusal the single removal gives; and it takes the
//! administration scope, like the single removal.

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, parked_harness, post_json, test_harness};

const FIRST: &str = "019d0000-0000-7000-8000-0000001140a4";
const SECOND: &str = "019d0000-0000-7000-8000-0000001140b4";
const ALL: &str = "/api/v1/plugins/superseded";

/// A loadable package for one version of `plugin`, as `plugin_versions::install_package` writes
/// one: a valid manifest and a component that compiles, unsigned, which the harness's
/// development-mode verifier accepts.
fn install_package(directory: &std::path::Path, plugin: &str, slug: &str, version: &str) {
    let path = directory.join("plugins").join(plugin).join(version);
    std::fs::create_dir_all(&path).expect("version directory");
    std::fs::write(
        path.join("manifest.toml"),
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.10.0"
id = "{plugin}"
name = "Superseded {slug}"
version = "{version}"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A superseded-versions fixture"
author = "Fixture Author"

[provider]
slug = "{slug}"
kind = "hoster"
credentials = "api_key"
"#
        ),
    )
    .expect("manifest");
    std::fs::write(path.join("component.wasm"), b"\0asm\x0d\0\x01\0").expect("component");
}

fn install_first(directory: &std::path::Path, versions: &[&str]) {
    for version in versions {
        install_package(directory, FIRST, "superseded_first", version);
    }
}

fn install_second(directory: &std::path::Path, versions: &[&str]) {
    for version in versions {
        install_package(directory, SECOND, "superseded_second", version);
    }
}

fn exists(directory: &std::path::Path, plugin: &str, version: &str) -> bool {
    directory
        .join("plugins")
        .join(plugin)
        .join(version)
        .is_dir()
}

/// The `plugin@version` names of one list of the answer, sorted.
fn named(body: &serde_json::Value, list: &str) -> Vec<String> {
    let mut names: Vec<String> = body[list]
        .as_array()
        .unwrap_or_else(|| panic!("{list} is a list: {body}"))
        .iter()
        .map(|entry| {
            format!(
                "{}@{}",
                entry["plugin_id"].as_str().unwrap_or_default(),
                entry["version"].as_str().unwrap_or_default()
            )
        })
        .collect();
    names.sort();
    names
}

fn of(plugin: &str, version: &str) -> String {
    format!("{plugin}@{version}")
}

/// The owner's request: one click and every leftover of every plugin is gone, audited, while
/// each plugin keeps the version that runs. A second click finds nothing.
#[tokio::test]
async fn every_superseded_version_goes_and_the_running_ones_stay() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_first(directory.path(), &["1.0.0", "2.0.0", "3.0.0"]);
    install_second(directory.path(), &["1.0.0", "2.0.0"]);

    let (status, body) = delete_json(&harness.router, ALL).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.superseded_removed", "{body}");
    assert_eq!(body["params"]["count"], "3", "{body}");
    assert_eq!(
        named(&body, "removed"),
        vec![of(FIRST, "1.0.0"), of(FIRST, "2.0.0"), of(SECOND, "1.0.0")]
    );
    assert!(named(&body, "kept").is_empty(), "{body}");
    assert!(exists(directory.path(), FIRST, "3.0.0"));
    assert!(exists(directory.path(), SECOND, "2.0.0"));
    for (plugin, version) in [(FIRST, "1.0.0"), (FIRST, "2.0.0"), (SECOND, "1.0.0")] {
        assert!(
            !exists(directory.path(), plugin, version),
            "{plugin} {version}"
        );
    }

    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::PluginRemoved),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    assert_eq!(records.len(), 3, "one record per removed version");
    assert!(
        records
            .iter()
            .all(|record| record.details.get("source").map(String::as_str) == Some("superseded"))
    );

    let (status, body) = delete_json(&harness.router, ALL).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.superseded_none", "{body}");
    assert!(named(&body, "removed").is_empty(), "{body}");
    assert!(named(&body, "kept").is_empty(), "{body}");
}

/// The card's own action: one plugin's leftovers go, every other plugin's stay.
#[tokio::test]
async fn one_plugins_superseded_versions_go_and_the_others_stay() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_first(directory.path(), &["1.0.0", "2.0.0"]);
    install_second(directory.path(), &["1.0.0", "2.0.0"]);

    let (status, body) = delete_json(
        &harness.router,
        &format!("/api/v1/plugins/{FIRST}/superseded"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(named(&body, "removed"), vec![of(FIRST, "1.0.0")]);
    assert!(!exists(directory.path(), FIRST, "1.0.0"));
    assert!(exists(directory.path(), FIRST, "2.0.0"));
    assert!(
        exists(directory.path(), SECOND, "1.0.0"),
        "another plugin's leftover is not this card's"
    );

    let (status, body) = delete_json(
        &harness.router,
        "/api/v1/plugins/019d0000-0000-7000-8000-0000001140ff/superseded",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin.not_installed");
}

/// A version a queued job is pinned to stays, named with the single removal's refusal; the
/// other leftover of the same plugin goes all the same.
#[tokio::test]
async fn a_version_unfinished_work_is_bound_to_stays_and_is_named() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    install_first(directory.path(), &["1.0.0", "2.0.0", "3.0.0"]);
    crate::plugin_versions::pinned_download_of(&harness.database, FIRST, "1.0.0").await;

    let (status, body) = delete_json(&harness.router, ALL).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.superseded_partly_removed", "{body}");
    assert_eq!(named(&body, "removed"), vec![of(FIRST, "2.0.0")]);
    assert_eq!(named(&body, "kept"), vec![of(FIRST, "1.0.0")]);
    let reason = &body["kept"][0]["reason"];
    assert_eq!(reason["code"], "plugin.version_in_use", "{body}");
    assert_eq!(reason["params"]["names"], "file.bin", "{body}");
    assert!(exists(directory.path(), FIRST, "1.0.0"));
    assert!(exists(directory.path(), FIRST, "3.0.0"));
}

/// The version that runs is never touched, and neither is a version that is chosen without
/// running yet: the one the next start loads — right after an update, the new one — and the one
/// under test. Both are named under `kept`.
#[tokio::test]
async fn the_loaded_version_never_goes_and_a_chosen_one_stays() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    install_first(directory.path(), &["1.0.0"]);
    harness
        .state
        .plugins
        .record_started_versions()
        .await
        .expect("the start records what it loads");
    install_first(directory.path(), &["2.0.0"]);
    let route = format!("/api/v1/plugins/{FIRST}/superseded");

    // 1.0.0 runs, 2.0.0 runs from the next start.
    let (status, body) = delete_json(&harness.router, &route).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(named(&body, "removed").is_empty(), "{body}");
    assert_eq!(named(&body, "kept"), vec![of(FIRST, "2.0.0")]);
    assert_eq!(
        body["kept"][0]["reason"]["code"],
        "plugin.version_next_start"
    );

    // Put under test, 2.0.0 is still chosen; the running 1.0.0 is the next start's again.
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/plugins/{FIRST}/lifecycle/stage"),
        serde_json::json!({ "version": "2.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = delete_json(&harness.router, &route).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(named(&body, "kept"), vec![of(FIRST, "2.0.0")]);
    assert_eq!(
        body["kept"][0]["reason"]["code"],
        "plugin.version_under_test"
    );

    assert!(exists(directory.path(), FIRST, "1.0.0"), "the loaded one");
    assert!(exists(directory.path(), FIRST, "2.0.0"), "the chosen one");
}

/// Removing takes the administration scope, like removing one version.
#[tokio::test]
async fn removing_superseded_versions_needs_the_admin_scope() {
    const CONFIG_BEARER: &str = "test-config-bearer-token";
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(
        directory.path(),
        common::Options::default()
            .login()
            .token(CONFIG_BEARER, "api:config"),
    )
    .await;
    install_first(directory.path(), &["1.0.0", "2.0.0"]);
    let one = format!("/api/v1/plugins/{FIRST}/superseded");

    for route in [ALL, one.as_str()] {
        let (status, body) =
            common::request_with_bearer(&harness.router, "DELETE", route, CONFIG_BEARER).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{route}: {body}");
    }
    assert!(exists(directory.path(), FIRST, "1.0.0"));

    let (status, body) =
        common::request_with_bearer(&harness.router, "DELETE", ALL, common::API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(named(&body, "removed"), vec![of(FIRST, "1.0.0")]);
}
