//! The repository first, the bundle as the fallback (RD-1260-02): a service the setup wizard or
//! the plugin manager installs comes from the plugin repository when it answers, and from the
//! bundle without a network; the catalogue says where each plugin comes from and lists what only
//! a repository offers.

use std::sync::Arc;

use axum::http::StatusCode;
use rd_plugin_host::{
    GeneratedKey, index,
    repository::{OFFICIAL_INDEX_URL, PluginRepositoryService, TrustStore},
};
use serde_json::{Value, json};

use crate::{
    common::{self, get_json, post_json},
    plugin_auto_updates::MapFetcher,
};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";
const KEY_ID: &str = "bundle-repository-fixture-v1";
const REPOSITORY_KEY_ID: &str = "bundle-repository-official";

/// A signed hoster package; `slug` keeps the provider rows of the cases apart.
fn package(author: &GeneratedKey, id: &str, slug: &str, version: &str) -> Vec<u8> {
    let manifest = format!(
        r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.11.0"
id = "{id}"
name = "Repository fixture {slug}"
version = "{version}"
key_id = "{KEY_ID}"
public_key = "{public_key}"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["{slug}.test"]

[metadata]
description = "A hoster in the bundle and in the repository"
author = "Fixture Author"

[provider]
slug = "{slug}"
kind = "hoster"
credentials = "api_key"
"#,
        public_key = author.public_base64,
    );
    rd_plugin_host::package_plugin(
        manifest.as_bytes(),
        EMPTY_COMPONENT,
        &[],
        Some(&author.signing_key),
    )
    .expect("package")
}

fn package_url(id: &str, version: &str) -> String {
    format!("https://plugins.example.test/{id}-{version}.rdplug")
}

/// The official index listing `packages` (`(id, version, bytes)`), signed with `repository`.
fn signed_index(repository: &GeneratedKey, packages: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let index = index::PluginIndex {
        schema_version: index::PLUGIN_INDEX_SCHEMA_VERSION,
        sequence: 7,
        issued_at: chrono::Utc::now() - chrono::Duration::minutes(1),
        not_after: chrono::Utc::now() + chrono::Duration::days(30),
        packages: packages
            .iter()
            .map(|(id, version, bytes)| {
                index::describe_package(bytes, package_url(id, version), None).expect("describe")
            })
            .collect(),
        revoked: index::Revocations::default(),
    };
    index::sign(REPOSITORY_KEY_ID, &repository.signing_key, &index).expect("sign")
}

struct Case {
    _directory: tempfile::TempDir,
    harness: common::Harness,
    router: axum::Router,
    author: GeneratedKey,
    repository: GeneratedKey,
    fetcher: Arc<MapFetcher>,
}

/// A harness whose bundle holds `id` at 1.0.0, signed by a trusted author, and whose official
/// repository is served by the fetcher under a test key.
async fn case(id: &str, slug: &str) -> Case {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let author = rd_plugin_host::generate_signing_key();
    let repository = rd_plugin_host::generate_signing_key();
    harness
        .state
        .plugins
        .verifier()
        .trust_key_base64(KEY_ID.to_owned(), &author.public_base64)
        .expect("trust the author");
    let bundle = directory.path().join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle dir");
    std::fs::write(
        bundle.join(format!("{slug}.rdplug")),
        package(&author, id, slug, "1.0.0"),
    )
    .expect("write");
    let report = rd_plugin_host::sync_bundled(
        &harness.state.plugins,
        &bundle,
        rd_plugin_host::BundledPolicy::InstalledOnly,
    )
    .await
    .expect("sync");
    assert_eq!(report.available, 1, "{:?}", report.rejected);

    let fetcher = Arc::new(MapFetcher::default());
    let trust = TrustStore::new();
    trust
        .trust(
            REPOSITORY_KEY_ID.to_owned(),
            repository.signing_key.verifying_key(),
        )
        .expect("trust the repository");
    let mut state = harness.state.clone();
    state.plugin_repositories = PluginRepositoryService::with_fetcher(
        harness.database.clone(),
        directory.path().join("plugin-repositories"),
        state.plugins.clone(),
        fetcher.clone(),
        Some(trust),
    );
    Case {
        router: rd_api::router(state),
        _directory: directory,
        harness,
        author,
        repository,
        fetcher,
    }
}

async fn install(router: &axum::Router, service: &str) -> Value {
    let (status, body) = post_json(
        router,
        "/api/v1/plugins/bundled/install",
        json!({ "services": [service] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["failed"], json!([]), "{body}");
    body
}

#[tokio::test]
async fn a_service_installs_from_the_repository_when_it_answers() {
    const ID: &str = "019d0000-0000-7000-8000-0000012602a1";
    const SLUG: &str = "repofirsta";
    let case = case(ID, SLUG).await;
    let newer = package(&case.author, ID, SLUG, "1.0.1");
    case.fetcher.serve(
        OFFICIAL_INDEX_URL,
        signed_index(&case.repository, &[(ID, "1.0.1", &newer)]),
    );
    case.fetcher.serve(&package_url(ID, "1.0.1"), newer);

    let body = install(&case.router, SLUG).await;
    assert_eq!(body["installed"][0]["origin"], "repository", "{body}");
    assert_eq!(body["installed"][0]["installed_version"], "1.0.1");
    let delivered = case
        .harness
        .database
        .list_plugin_repository_installs()
        .await
        .expect("deliveries");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].version, "1.0.1");

    let (status, catalogue) = get_json(&case.router, "/api/v1/plugins/bundled?locale=en").await;
    assert_eq!(status, StatusCode::OK, "{catalogue}");
    let plugin = &catalogue["services"][0]["plugins"][0];
    assert_eq!(catalogue["services"][0]["state"], "installed");
    assert_eq!(plugin["installed_origin"], "repository", "{catalogue}");
}

/// The case the bundle exists for: no network, and every service still installs.
#[tokio::test]
async fn without_a_network_a_service_installs_from_the_bundle() {
    const ID: &str = "019d0000-0000-7000-8000-0000012602b2";
    const SLUG: &str = "repofirstb";
    let case = case(ID, SLUG).await;

    let body = install(&case.router, SLUG).await;
    assert_eq!(body["installed"][0]["origin"], "bundled", "{body}");
    assert_eq!(body["installed"][0]["installed_version"], "1.0.0");
    let (_, catalogue) = get_json(&case.router, "/api/v1/plugins/bundled?locale=en").await;
    let plugin = &catalogue["services"][0]["plugins"][0];
    assert_eq!(plugin["installed_origin"], "bundled", "{catalogue}");
    assert_eq!(plugin["origin"], "bundled");
}

/// One view: the bundle's services with where each plugin would come from, and beside them what
/// only the repository offers, to install through the preview.
#[tokio::test]
async fn the_catalogue_names_the_origin_and_lists_what_only_a_repository_offers() {
    const ID: &str = "019d0000-0000-7000-8000-0000012602c3";
    const ONLY: &str = "019d0000-0000-7000-8000-0000012602c4";
    const SLUG: &str = "repofirstc";
    let case = case(ID, SLUG).await;
    let same = package(&case.author, ID, SLUG, "1.0.0");
    let only = package(&case.author, ONLY, "repofirstonly", "2.0.0");
    case.fetcher.serve(
        OFFICIAL_INDEX_URL,
        signed_index(
            &case.repository,
            &[(ID, "1.0.0", &same), (ONLY, "2.0.0", &only)],
        ),
    );
    let (status, body) = post_json(
        &case.router,
        "/api/v1/plugins/repositories/refresh",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, catalogue) = get_json(&case.router, "/api/v1/plugins/bundled?locale=en").await;
    assert_eq!(status, StatusCode::OK, "{catalogue}");
    let plugin = &catalogue["services"][0]["plugins"][0];
    assert_eq!(plugin["origin"], "repository", "{catalogue}");
    assert_eq!(plugin["repository_version"], "1.0.0");
    assert_eq!(plugin["repository_id"], "official");
    assert!(plugin["installed_origin"].is_null());
    let only = &catalogue["repository_services"];
    assert_eq!(only.as_array().map(Vec::len), Some(1), "{catalogue}");
    assert_eq!(only[0]["key"], ONLY);
    assert_eq!(only[0]["state"], "available");
    assert_eq!(only[0]["plugins"][0]["origin"], "repository");
    assert_eq!(only[0]["plugins"][0]["version"], "2.0.0");
}
