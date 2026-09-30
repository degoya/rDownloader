//! Removing a bundled service (RD-180-14).
//!
//! The owner's request: a fresh installation starts with every service that needs no account,
//! and unticking one in the setup wizard's "Your services" step removes it. What these hold:
//! every plugin of the service goes, audited; the next start does not bring it back; a version
//! an unfinished download is bound to keeps its whole service while the others go; and removing
//! takes the administration scope, like installing.

use crate::common;
use crate::plugin_bundled::{DEV_PUBLIC_KEY, SLUG, offer_bundle, offer_packages, provider_offered};

use axum::http::StatusCode;
use common::{get_json, parked_harness, post_json, test_harness};

const CHECKSUMS: &str = "bundlechecksums";

/// A post-processing step: a service of its own that needs no account (RD-180-14).
fn checksums_manifest() -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "postprocess"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000180e2"
name = "Bundle Checksums"
version = "1.0.0"
key_id = "dev"
public_key = "{DEV_PUBLIC_KEY}"

[metadata]
description = "Verifies a sidecar"
author = "Fixture Author"

[extension]
slug = "{CHECKSUMS}"
claims = []
"#
    )
}

async fn install(router: &axum::Router, services: &[&str]) {
    let (status, body) = post_json(
        router,
        "/api/v1/plugins/bundled/install",
        serde_json::json!({ "services": services }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["failed"].as_array().map(Vec::len), Some(0), "{body}");
}

async fn installed_count(router: &axum::Router) -> Option<usize> {
    let (_, inventory) = get_json(router, "/api/v1/plugins").await;
    inventory["installed"].as_array().map(Vec::len)
}

/// The owner's request (RD-180-14): unticking a service in the wizard removes it — every plugin
/// of it, audited — and the next start does not bring it back.
#[tokio::test]
async fn removing_a_service_removes_all_of_its_plugins_for_good() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    offer_bundle(&harness, directory.path()).await;
    install(&harness.router, &[SLUG]).await;
    assert_eq!(installed_count(&harness.router).await, Some(2));

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/remove",
        serde_json::json!({ "services": [SLUG] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.bundled_removed", "{body}");
    assert_eq!(body["removed"], serde_json::json!([SLUG]), "{body}");
    assert_eq!(body["failed"].as_array().map(Vec::len), Some(0), "{body}");
    assert_eq!(
        installed_count(&harness.router).await,
        Some(0),
        "the sign-in went with its hoster"
    );
    let (_, providers) = get_json(&harness.router, "/api/v1/providers").await;
    assert!(!provider_offered(&providers), "{providers}");
    let (_, catalogue) = get_json(&harness.router, "/api/v1/plugins/bundled").await;
    assert_eq!(
        catalogue["services"][0]["state"], "available",
        "{catalogue}"
    );

    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::PluginRemoved),
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    assert_eq!(records.len(), 2, "one record per removed plugin");
    assert!(
        records
            .iter()
            .all(|record| record.details.get("source").map(String::as_str) == Some("bundled"))
    );

    // The next start: once the first one ran, a start only updates what is installed.
    let report = rd_plugin_host::sync_bundled(
        &harness.state.plugins,
        &directory.path().join("bundle"),
        rd_plugin_host::BundledPolicy::InstalledOnly,
    )
    .await
    .expect("sync");
    assert!(report.installed.is_empty(), "{:?}", report.installed);
    assert_eq!(installed_count(&harness.router).await, Some(0));

    // A service that is not installed is skipped, not refused.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/remove",
        serde_json::json!({ "services": [SLUG] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], serde_json::json!([]), "{body}");
    assert_eq!(body["failed"].as_array().map(Vec::len), Some(0), "{body}");
}

/// A version an unfinished download is bound to keeps its whole service, with the same refusal
/// the plugin manager's delete gives; the other services named are removed all the same.
#[tokio::test]
async fn a_service_a_download_is_bound_to_stays_and_the_rest_go() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    offer_packages(
        &harness,
        directory.path(),
        vec![("checksums.rdplug", checksums_manifest())],
    )
    .await;
    install(&harness.router, &[SLUG, CHECKSUMS]).await;
    assert_eq!(installed_count(&harness.router).await, Some(3));
    crate::plugin_versions::pinned_download_of(
        &harness.database,
        "019d0000-0000-7000-8000-0000000160a1",
        "1.0.0",
    )
    .await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/remove",
        serde_json::json!({ "services": [SLUG, CHECKSUMS] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.bundled_partly_removed", "{body}");
    assert_eq!(body["removed"], serde_json::json!([CHECKSUMS]), "{body}");
    let failed = body["failed"].as_array().expect("failed");
    assert_eq!(failed.len(), 1, "{body}");
    assert_eq!(failed[0]["service"], SLUG);
    assert_eq!(failed[0]["code"], "plugin.version_in_use");
    assert_eq!(
        installed_count(&harness.router).await,
        Some(2),
        "neither the hoster nor its sign-in was removed"
    );
}

/// Removing takes the administration scope, like installing.
#[tokio::test]
async fn removing_a_service_needs_the_admin_scope() {
    const CONFIG_BEARER: &str = "test-config-bearer-token";
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::harness(
        directory.path(),
        common::Options::default()
            .login()
            .token(CONFIG_BEARER, "api:config"),
    )
    .await;
    offer_bundle(&harness, directory.path()).await;
    let (status, body) = common::post_with_bearer(
        &harness.router,
        "/api/v1/plugins/bundled/remove",
        CONFIG_BEARER,
        serde_json::json!({ "services": [SLUG] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // With the administration scope the request reaches the service list.
    let (status, body) = common::post_with_bearer(
        &harness.router,
        "/api/v1/plugins/bundled/remove",
        common::API_BEARER,
        serde_json::json!({ "services": ["no_such_service"] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin.bundled_service_unknown");
}
