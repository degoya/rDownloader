//! The bundled plugins by service, and installing a chosen one (RD-160-05).
//!
//! Which packages a start installs is tested in `rd_plugin_host::bundled`; these tests hold the
//! REST contract the setup wizard and the plugin manager build on: an unchosen service is
//! listed as available and offers no provider, installing it takes one request and makes its
//! provider row live at once — so the accounts step can offer it without a restart — and a
//! request naming a service the bundle does not have installs nothing. A first install runs at
//! once (RD-170-12): the account check right after it reaches the plugin, and one that could not
//! load says it runs after a restart.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_harness};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";
const DEV_PUBLIC_KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";
/// Unique to this suite: the provider registry is one per process.
const SLUG: &str = "bundlefixture";

fn resolver_manifest() -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000160a1"
name = "Bundle Fixture"
version = "1.0.0"
key_id = "dev"
public_key = "{DEV_PUBLIC_KEY}"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["bundle-fixture.test"]

[metadata]
description = "A hoster that ships in the bundle"
author = "Fixture Author"

[provider]
slug = "{SLUG}"
kind = "hoster"
credentials = "api_key"
"#
    )
}

/// The sign-in of the same service: it claims the provider, so it installs with it.
fn auth_manifest() -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "auth"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000160a2"
name = "Bundle Fixture sign-in"
version = "1.0.0"
key_id = "dev"
public_key = "{DEV_PUBLIC_KEY}"

[metadata]
description = "Signs in at the bundle fixture"
author = "Fixture Author"

[extension]
slug = "bundlefixture_auth"
claims = ["{SLUG}"]
"#
    )
}

/// Writes the bundle directory and lets the start-up sync read it, installing nothing new —
/// the state of every start after the first.
async fn offer_bundle(harness: &common::Harness, directory: &std::path::Path) {
    let bundle = directory.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    for (name, manifest) in [
        ("fixture.rdplug", resolver_manifest()),
        ("fixture-auth.rdplug", auth_manifest()),
    ] {
        let package =
            rd_plugin_host::package_plugin(manifest.as_bytes(), EMPTY_COMPONENT, &[], None)
                .expect("package");
        std::fs::write(bundle.join(name), package).expect("write");
    }
    let report = rd_plugin_host::sync_bundled(
        &harness.state.plugins,
        &bundle,
        rd_plugin_host::BundledPolicy::InstalledOnly,
    )
    .await
    .expect("sync");
    assert_eq!(report.available, 2, "{:?}", report.rejected);
}

fn provider_offered(providers: &serde_json::Value) -> bool {
    providers
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["slug"] == SLUG))
}

#[tokio::test]
async fn an_unchosen_service_is_available_and_one_request_installs_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    offer_bundle(&harness, directory.path()).await;

    let (status, body) = get_json(&harness.router, "/api/v1/plugins/bundled?locale=de").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let services = body["services"].as_array().expect("services");
    assert_eq!(
        services.len(),
        1,
        "the sign-in belongs to its provider: {body}"
    );
    let service = &services[0];
    assert_eq!(service["key"], SLUG);
    assert_eq!(service["name"], "Bundle Fixture");
    assert_eq!(service["category"], "hoster");
    assert_eq!(service["needs_account"], true);
    assert_eq!(service["provider"], SLUG);
    assert_eq!(service["state"], "available");
    assert_eq!(service["plugins"].as_array().map(Vec::len), Some(2));

    // Not installed, so the accounts step has nothing to offer for it.
    let (_, providers) = get_json(&harness.router, "/api/v1/providers").await;
    assert!(!provider_offered(&providers), "{providers}");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/install",
        serde_json::json!({ "services": [SLUG] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The fixture's components cannot run, so both wait for a restart and the answer says so.
    assert_eq!(
        body["code"], "plugin.bundled_installed_restart_required",
        "{body}"
    );
    assert_eq!(body["restart_required"], true, "{body}");
    assert_eq!(body["installed"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["failed"].as_array().map(Vec::len), Some(0));

    // The provider row is live at once: the accounts step offers it before any restart.
    let (_, providers) = get_json(&harness.router, "/api/v1/providers").await;
    assert!(provider_offered(&providers), "{providers}");
    let (_, body) = get_json(&harness.router, "/api/v1/plugins/bundled").await;
    assert_eq!(body["services"][0]["state"], "installed", "{body}");
    let (_, inventory) = get_json(&harness.router, "/api/v1/plugins").await;
    assert_eq!(inventory["installed"].as_array().map(Vec::len), Some(2));

    // An account of the provider is told its plugin runs after a restart — not that no resolver
    // is installed, which is what the owner read right after the wizard installed it.
    let account = harness
        .database
        .create_account(rd_db::NewAccount {
            provider: SLUG.to_owned(),
            label: "Fixture".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/accounts/{}/test", account.id),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "plugin.installed_not_running", "{body}");

    // Asking again installs nothing twice.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/install",
        serde_json::json!({ "services": [SLUG] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["installed"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn a_service_the_bundle_does_not_have_installs_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    offer_bundle(&harness, directory.path()).await;

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/install",
        serde_json::json!({ "services": [SLUG, "no_such_service"] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin.bundled_service_unknown");
    let (_, inventory) = get_json(&harness.router, "/api/v1/plugins").await;
    assert_eq!(
        inventory["installed"].as_array().map(Vec::len),
        Some(0),
        "every key is checked before anything installs"
    );

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/install",
        serde_json::json!({ "services": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "plugin.bundled_services_invalid");
}

/// The shipped DDownload package with its built component, signed by nobody — the harness
/// verifies in development mode — under the development key.
fn ddownload_package() -> Vec<u8> {
    let root = std::path::Path::new("../../plugins/ddownload");
    let manifest = std::fs::read_to_string(root.join("manifest.toml"))
        .expect("the ddownload manifest")
        .lines()
        .map(|line| {
            if line.starts_with("key_id = ") {
                "key_id = \"dev\"".to_owned()
            } else if line.starts_with("public_key = ") {
                format!("public_key = \"{DEV_PUBLIC_KEY}\"")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut locales: Vec<(String, Vec<u8>)> = std::fs::read_dir(root.join("locales"))
        .expect("the ddownload locales")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let language = name.strip_suffix(".json")?.to_owned();
            Some((language, std::fs::read(entry.path()).ok()?))
        })
        .collect();
    locales.sort_by(|left, right| left.0.cmp(&right.0));
    rd_plugin_host::package_plugin(
        manifest.as_bytes(),
        &rd_plugin_host::artifact::component("rd-plugin-ddownload"),
        &locales,
        None,
    )
    .expect("package")
}

/// The owner's report (RD-170-12): the wizard installed DDownload, the accounts step offered it,
/// and the account's check failed with "No resolver is installed for this provider" because the
/// resolvers were built at the start. Installed from the bundle, it runs at once now: the
/// plugin manager lists it as running, and the account check reaches the plugin itself.
#[tokio::test]
async fn a_bundled_resolver_runs_at_once_on_real_components() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    // What a start does: nothing is installed yet, and that is what it loaded.
    harness
        .state
        .plugins
        .record_started_versions()
        .await
        .expect("the start records what it loads");
    let bundle = directory.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    std::fs::write(bundle.join("ddownload.rdplug"), ddownload_package()).expect("write");
    let report = rd_plugin_host::sync_bundled(
        &harness.state.plugins,
        &bundle,
        rd_plugin_host::BundledPolicy::InstalledOnly,
    )
    .await
    .expect("sync");
    assert_eq!(report.available, 1, "{:?}", report.rejected);
    let url: url::Url = "https://ddownload.com/abc123xyz/release.rar"
        .parse()
        .expect("URL");
    assert!(!harness.scheduler.resolvers().has_resolver(&url));

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/plugins/bundled/install",
        serde_json::json!({ "services": ["ddownload"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "plugin.bundled_installed", "{body}");
    assert_eq!(body["restart_required"], false, "{body}");
    assert!(harness.scheduler.resolvers().has_resolver(&url));

    // The plugin manager lists it as running, with nothing waiting for a restart.
    let (_, inventory) = get_json(&harness.router, "/api/v1/plugins").await;
    let lifecycle = inventory["lifecycle"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["plugin_id"] == "019d0000-0000-7000-8000-000000000001")
        })
        .cloned()
        .expect("a lifecycle entry for DDownload");
    assert!(lifecycle["running_version"].is_string(), "{lifecycle}");
    assert_eq!(lifecycle["running_version"], lifecycle["active_version"]);
    assert_eq!(lifecycle["restart_required"], false, "{lifecycle}");

    // The account check reaches the plugin: an account without any credential is the plugin's
    // own refusal, asked of no server, and neither "no resolver" nor "after a restart".
    let account = harness
        .database
        .create_account(rd_db::NewAccount {
            provider: "ddownload".to_owned(),
            label: "DDownload".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account");
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/accounts/{}/test", account.id),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"], "account.check_failed", "{body}");
    let reason = body["params"]["reason"].as_str().unwrap_or_default();
    assert!(!reason.contains("No resolver"), "{body}");
    assert!(reason.to_lowercase().contains("cookie"), "{body}");
}
