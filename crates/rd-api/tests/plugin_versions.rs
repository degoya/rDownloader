//! Removing the version an upgrade left behind, and refusing to remove one that is still in use.
//!
//! Installing a plugin never removes the older version: a job already under way keeps the
//! version that started it, which is the whole point of the resolver pin and of the transfer
//! checkpoint. Nothing ever cleared those leftovers, so the manager offers it now (RD-108-10) --
//! and has to answer for the case the leftover exists for.

mod common;

use axum::http::StatusCode;
use common::{delete_json, get_json, parked_harness, post_json, put_json, test_harness};
use rd_plugin_host::repository::{UpdatePolicy, UpdatePolicySource};

const PLUGIN: &str = "019d0000-0000-7000-8000-000000000108";

/// Two installed version directories, as an upgrade leaves them. The manifests are not read
/// here: removal works on the directory, and the listing skips what it cannot parse.
fn install_versions(directory: &std::path::Path, versions: &[&str]) {
    for version in versions {
        std::fs::create_dir_all(directory.join("plugins").join(PLUGIN).join(version))
            .expect("version directory");
    }
}

fn version_exists(directory: &std::path::Path, version: &str) -> bool {
    directory
        .join("plugins")
        .join(PLUGIN)
        .join(version)
        .is_dir()
}

/// Enqueues one download and pins it to `version`, the way resolving does.
///
/// Only against a parked harness: a live scheduler claims the queued row and resolves it for
/// real, which races the pin and every state the test then sets by hand.
async fn pinned_download(database: &rd_db::Database, version: &str) -> rd_core::DownloadId {
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "plugin versions".to_owned(),
            destination: "downloads".to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: "https://example.test/file.bin".parse().expect("URL"),
            file_name: "file.bin".to_owned(),
            total_bytes: None,
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
        })
        .await
        .expect("download");
    database
        .claim_resolver_pin(
            download.id,
            rd_core::ResolverPin {
                plugin_id: PLUGIN.parse().expect("plugin id"),
                version: version.to_owned(),
            },
        )
        .await
        .expect("pin");
    download.id
}

#[tokio::test]
async fn a_version_a_job_is_pinned_to_is_not_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    install_versions(directory.path(), &["1.0.0", "2.0.0"]);
    let download = pinned_download(&harness.database, "1.0.0").await;

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.version_in_use");
    assert_eq!(
        body["params"]["count"], "1",
        "the refusal says how much is bound"
    );
    assert_eq!(
        body["params"]["names"], "file.bin",
        "and names the job in the way, so it can be found"
    );
    assert!(
        version_exists(directory.path(), "1.0.0"),
        "the package a queued job needs is still on disk"
    );

    // Cancelling is not the end of the job: `ProgressControl::cancel` keeps the partial data
    // and `resume` puts it back in the queue, so the version it would resume with stays.
    harness
        .database
        .transition_download(download, rd_core::DownloadState::Cancelled)
        .await
        .expect("cancel");

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.version_in_use");
    assert!(version_exists(directory.path(), "1.0.0"));

    // Deleting the job is what releases the claim, because deleting takes the pin with it.
    harness
        .database
        .delete_download(download)
        .await
        .expect("delete");

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !version_exists(directory.path(), "1.0.0"),
        "the leftover is gone"
    );
    assert!(
        version_exists(directory.path(), "2.0.0"),
        "the version that is loaded stays"
    );
}

#[tokio::test]
async fn the_version_that_is_loaded_is_guarded_the_same_way() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    install_versions(directory.path(), &["1.0.0", "2.0.0"]);
    pinned_download(&harness.database, "2.0.0").await;

    // Nothing is bound to the older one, so it goes.
    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The guard is about the version a job named, not about which one happens to be newest.
    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/2.0.0")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.version_in_use");
    assert!(version_exists(directory.path(), "2.0.0"));
}

#[tokio::test]
async fn removing_a_version_nothing_is_bound_to_still_answers_not_installed_twice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_versions(directory.path(), &["1.0.0"]);

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) =
        delete_json(&harness.router, &format!("/api/v1/plugins/{PLUGIN}/1.0.0")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin.not_installed");
}

/// A loadable package for one version: a valid manifest and a component that compiles. Unsigned,
/// which the harness's development-mode verifier accepts.
fn install_package(directory: &std::path::Path, version: &str) {
    let path = directory.join("plugins").join(PLUGIN).join(version);
    std::fs::create_dir_all(&path).expect("version directory");
    std::fs::write(
        path.join("manifest.toml"),
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "{PLUGIN}"
name = "Lifecycle"
version = "{version}"
key_id = "fixture-v1"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A lifecycle fixture"
author = "Fixture Author"

[provider]
slug = "lifecycle"
kind = "hoster"
credentials = "api_key"
"#
        ),
    )
    .expect("manifest");
    std::fs::write(path.join("component.wasm"), b"\0asm\x0d\0\x01\0").expect("component");
}

async fn lifecycle(router: &axum::Router) -> serde_json::Value {
    let (status, body) = get_json(router, "/api/v1/plugins").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["lifecycle"]
        .as_array()
        .expect("lifecycle list")
        .iter()
        .find(|entry| entry["plugin_id"] == PLUGIN)
        .cloned()
        .expect("an entry for the plugin")
}

fn lifecycle_route(action: &str) -> String {
    format!("/api/v1/plugins/{PLUGIN}/lifecycle/{action}")
}

/// RD-140-02's state machine through the REST surface: every action moves the pointers it
/// names, takes effect at the next start, and refuses what the next start could not honour.
#[tokio::test]
async fn activate_stage_and_roll_back_move_the_version_pointers() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    install_package(directory.path(), "1.0.0");
    install_package(directory.path(), "2.0.0");
    let router = &harness.router;

    // Without a choice the newest version runs, exactly as before.
    let entry = lifecycle(router).await;
    assert_eq!(entry["active_version"], "2.0.0");
    assert_eq!(entry["running_version"], "2.0.0");
    assert_eq!(entry["update_policy"], "manual");
    assert_eq!(entry["restart_required"], false);

    // The version that runs cannot also be the one under test.
    let (status, body) = post_json(
        router,
        &lifecycle_route("stage"),
        serde_json::json!({ "version": "2.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.version_already_active");

    // Back to 1.0.0: stored now, running at the next start.
    let (status, body) = post_json(
        router,
        &lifecycle_route("activate"),
        serde_json::json!({ "version": "1.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.version_activated");
    let entry = lifecycle(router).await;
    assert_eq!(entry["active_version"], "1.0.0");
    assert_eq!(entry["previous_version"], "2.0.0");
    assert_eq!(entry["running_version"], "2.0.0");
    assert_eq!(entry["restart_required"], true);

    // 2.0.0 under test next to the active 1.0.0.
    let (status, body) = post_json(
        router,
        &lifecycle_route("stage"),
        serde_json::json!({ "version": "2.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry = lifecycle(router).await;
    assert_eq!(entry["active_version"], "1.0.0");
    assert_eq!(entry["staged_version"], "2.0.0");

    // Trying it on a download needs the staged version loaded, which takes a restart.
    let download = pinned_download(&harness.database, "1.0.0").await;
    let (status, body) = post_json(
        router,
        &lifecycle_route("trial"),
        serde_json::json!({ "download_id": download }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.staging_restart_required");
    assert_eq!(
        harness
            .database
            .resolver_pin(download)
            .await
            .expect("pin")
            .map(|pin| pin.version),
        Some("1.0.0".to_owned()),
        "a refused trial leaves the download's pin alone"
    );

    // Rolling back returns to 2.0.0 in one step, and ends its test on the way.
    let (status, body) =
        post_json(router, &lifecycle_route("rollback"), serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let entry = lifecycle(router).await;
    assert_eq!(entry["active_version"], "2.0.0");
    assert_eq!(entry["previous_version"], "1.0.0");
    assert!(entry["staged_version"].is_null());

    let (status, body) = delete_json(router, &lifecycle_route("stage")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin.nothing_staged");

    let (status, body) = put_json(
        router,
        &lifecycle_route("policy"),
        serde_json::json!({ "policy": "automatic" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(lifecycle(router).await["update_policy"], "automatic");
    // The repository refresh reads the same choice (RD-140-01).
    let source = rd_api::VersionChoicePolicy::new(harness.database.clone());
    assert_eq!(source.policy(PLUGIN).await, UpdatePolicy::Automatic);
    assert_eq!(
        source.policy("019d0000-0000-7000-8000-00000000ffff").await,
        UpdatePolicy::Manual,
        "a plugin without a choice updates by hand"
    );

    // Every stored choice is audited, oldest first here; the refused ones left no record.
    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::PluginVersionChosen),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    let choices: Vec<(&str, Option<&str>)> = records
        .iter()
        .rev()
        .map(|record| {
            assert_eq!(record.target_id.as_deref(), Some(PLUGIN));
            (
                record.details["choice"].as_str(),
                record.details.get("version").map(String::as_str),
            )
        })
        .collect();
    assert_eq!(
        choices,
        [
            ("activated", Some("1.0.0")),
            ("staged", Some("2.0.0")),
            ("rolled_back", Some("2.0.0")),
            ("update_policy", None),
        ]
    );

    let (status, body) = post_json(
        router,
        &lifecycle_route("activate"),
        serde_json::json!({ "version": "9.9.9" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin.version_not_installed");
}

/// A withdrawn version can be neither activated nor staged, and a rollback does not return to
/// one: the check before a choice is the check the next start makes.
#[tokio::test]
async fn a_withdrawn_version_cannot_be_chosen() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_package(directory.path(), "1.0.0");
    install_package(directory.path(), "2.0.0");
    let router = &harness.router;

    let (status, body) = post_json(
        router,
        &lifecycle_route("activate"),
        serde_json::json!({ "version": "1.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = post_json(
        router,
        "/api/v1/plugins/revocations",
        serde_json::json!({ "plugin_id": PLUGIN, "version": "2.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    for action in ["activate", "stage"] {
        let (status, body) = post_json(
            router,
            &lifecycle_route(action),
            serde_json::json!({ "version": "2.0.0" }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{action}: {body}");
        assert_eq!(body["code"], "plugin.version_unhealthy", "{action}");
    }
    // 2.0.0 was the rollback target; withdrawn, it is no target at all.
    let entry = lifecycle(router).await;
    assert!(entry["previous_version"].is_null(), "{entry}");
    let (status, body) =
        post_json(router, &lifecycle_route("rollback"), serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "plugin.no_previous_version");
}

/// Removing a version takes every pointer at it along.
#[tokio::test]
async fn removing_a_version_clears_the_pointers_at_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    install_package(directory.path(), "1.0.0");
    install_package(directory.path(), "2.0.0");
    let router = &harness.router;
    let (status, body) = post_json(
        router,
        &lifecycle_route("activate"),
        serde_json::json!({ "version": "1.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = delete_json(router, &format!("/api/v1/plugins/{PLUGIN}/2.0.0")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let stored = harness
        .database
        .plugin_version_choice(PLUGIN)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(stored.active_version.as_deref(), Some("1.0.0"));
    assert_eq!(stored.previous_version, None);
}
