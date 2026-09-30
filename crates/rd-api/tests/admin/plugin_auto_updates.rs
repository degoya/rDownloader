//! The repository refresh with its automatic updates (RD-160-09): `refresh_and_update`, which
//! the refresh route and the background loop both run, against an in-memory fetcher.
//!
//! What it fetches, verifies and offers is tested in `rd_plugin_host::repository`; what it
//! installs is decided here, in the API layer, and so is tested here.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, put_json};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

/// Serves whatever the test put in; every other address is offline.
#[derive(Default)]
struct MapFetcher(std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>);

impl MapFetcher {
    fn serve(&self, url: &str, bytes: Vec<u8>) {
        self.0.lock().expect("lock").insert(url.to_owned(), bytes);
    }

    fn go_offline(&self, url: &str) {
        self.0.lock().expect("lock").remove(url);
    }
}

type Fetched<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send + 'a>>;

// Spelled out as `async_trait` expands it, which this test crate does not depend on.
impl rd_plugin_host::repository::Fetcher for MapFetcher {
    fn fetch<'life0, 'life1, 'async_trait>(
        &'life0 self,
        url: &'life1 url::Url,
        limit: u64,
    ) -> Fetched<'async_trait>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        let served = self.0.lock().expect("lock").get(url.as_str()).cloned();
        let url = url.to_string();
        Box::pin(async move {
            let bytes = served.ok_or_else(|| anyhow::anyhow!("offline: {url}"))?;
            anyhow::ensure!(bytes.len() as u64 <= limit, "larger than {limit}");
            Ok(bytes)
        })
    }
}

const AUTOMATIC: &str = "019d0000-0000-7000-8000-0000001609a1";
const WIDER: &str = "019d0000-0000-7000-8000-0000001609b2";
const MANUAL: &str = "019d0000-0000-7000-8000-0000001609c3";
const GOOD_INDEX: &str = "https://plugins.example.test/rdownloader-plugin-index.json";
const GONE_INDEX: &str = "https://gone.example.test/rdownloader-plugin-index.json";

/// A signed package of one of the three plugins below, asking for `domains`.
fn update_fixture(
    author: &rd_plugin_host::GeneratedKey,
    id: &str,
    version: &str,
    domains: &str,
) -> Vec<u8> {
    let slug = format!("autoupdate{}", &id[id.len() - 2..]);
    let manifest = format!(
        r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "{id}"
name = "Update fixture {slug}"
version = "{version}"
key_id = "update-fixture-v1"
public_key = "{public_key}"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = [{domains}]

[metadata]
description = "A package the refresh may or may not install"
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

/// An index of `packages` (`(id, version, bytes)`), signed with the repository key.
fn signed_index(
    repository: &rd_plugin_host::GeneratedKey,
    sequence: u64,
    packages: &[(&str, &str, &[u8])],
) -> Vec<u8> {
    use rd_plugin_host::index;
    let index = index::PluginIndex {
        schema_version: index::PLUGIN_INDEX_SCHEMA_VERSION,
        sequence,
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
    index::sign("update-repository-v1", &repository.signing_key, &index).expect("sign")
}

/// Adds a third-party repository the way the settings page does: refused once with the
/// fingerprint, then confirmed with it.
async fn add_repository(router: &axum::Router, url: &str, public_key: &str) {
    let request = serde_json::json!({ "url": url, "public_key": public_key });
    let (status, body) = post_json(router, "/api/v1/plugins/repositories", request.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let fingerprint = body["params"]["fingerprint"]
        .as_str()
        .expect("the fingerprint to confirm")
        .to_owned();
    let (status, body) = post_json(
        router,
        &format!("/api/v1/plugins/repositories?trust_fingerprint={fingerprint}"),
        request,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// RD-160-09: the refresh the background loop runs installs the update of a plugin set to
/// automatic, leaves one that asks for a new permission and one set to manual for a click, and
/// neither the official repository nor a repository that went offline stops it.
#[tokio::test]
async fn the_refresh_installs_only_automatic_updates_that_ask_for_nothing_new() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let repository = rd_plugin_host::generate_signing_key();
    let author = rd_plugin_host::generate_signing_key();
    harness
        .state
        .plugins
        .verifier()
        .trust_key_base64("update-fixture-v1".to_owned(), &author.public_base64)
        .expect("trust the author");
    for id in [AUTOMATIC, WIDER, MANUAL] {
        harness
            .state
            .plugins
            .install_bytes(update_fixture(&author, id, "1.0.0", r#""example.test""#))
            .await
            .expect("install 1.0.0");
    }
    harness
        .state
        .plugins
        .record_started_versions()
        .await
        .expect("the start records what it loads");

    let fetcher = std::sync::Arc::new(MapFetcher::default());
    let mut state = harness.state.clone();
    state.plugin_repositories = rd_plugin_host::repository::PluginRepositoryService::with_fetcher(
        harness.database.clone(),
        directory.path().join("plugin-repositories"),
        state.plugins.clone(),
        fetcher.clone(),
        None,
    );
    state
        .plugin_repositories
        .set_update_policy(std::sync::Arc::new(rd_api::VersionChoicePolicy::new(
            harness.database.clone(),
        )));
    let router = rd_api::router(state);

    for id in [AUTOMATIC, WIDER] {
        let (status, body) = put_json(
            &router,
            &format!("/api/v1/plugins/{id}/lifecycle/policy"),
            serde_json::json!({ "policy": "automatic" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    fetcher.serve(GOOD_INDEX, signed_index(&repository, 1, &[]));
    fetcher.serve(GONE_INDEX, signed_index(&repository, 1, &[]));
    add_repository(&router, GOOD_INDEX, &repository.public_base64).await;
    add_repository(&router, GONE_INDEX, &repository.public_base64).await;

    // The updates arrive with the next index, so only a refresh that reached it installs one.
    let same = update_fixture(&author, AUTOMATIC, "1.1.0", r#""example.test""#);
    let wider = update_fixture(
        &author,
        WIDER,
        "1.1.0",
        r#""example.test", "more.example.test""#,
    );
    let manual = update_fixture(&author, MANUAL, "1.1.0", r#""example.test""#);
    for (id, bytes) in [(AUTOMATIC, &same), (WIDER, &wider), (MANUAL, &manual)] {
        fetcher.serve(&package_url(id, "1.1.0"), bytes.clone());
    }
    fetcher.serve(
        GOOD_INDEX,
        signed_index(
            &repository,
            2,
            &[
                (AUTOMATIC, "1.1.0", same.as_slice()),
                (WIDER, "1.1.0", wider.as_slice()),
                (MANUAL, "1.1.0", manual.as_slice()),
            ],
        ),
    );
    fetcher.go_offline(GONE_INDEX);

    let (status, body) = post_json(
        &router,
        "/api/v1/plugins/repositories/refresh",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a failed fetch is no failed refresh: {body}"
    );
    let repositories = body["repositories"].as_array().expect("repositories");
    let by_url = |url: &str| {
        repositories
            .iter()
            .find(|row| row["url"] == url)
            .cloned()
            .expect("the repository")
    };
    assert!(!by_url(GONE_INDEX)["last_error"].is_null(), "{body}");
    assert!(by_url(GOOD_INDEX)["last_error"].is_null(), "{body}");
    assert_eq!(by_url(GOOD_INDEX)["sequence"], 2, "{body}");

    let installed = |id: &str| {
        directory
            .path()
            .join("plugins")
            .join(id)
            .join("1.1.0")
            .is_dir()
    };
    assert!(
        installed(AUTOMATIC),
        "the automatic update was not installed"
    );
    assert!(
        !installed(WIDER),
        "an update with a new domain installed itself"
    );
    assert!(!installed(MANUAL), "a plugin set to manual updated itself");

    // What is left waits in the list, the new domain named; the installed update runs from
    // the next start.
    let (status, offers) = get_json(&router, "/api/v1/plugins/updates").await;
    assert_eq!(status, StatusCode::OK, "{offers}");
    let updates = offers["updates"].as_array().expect("updates");
    let listed: Vec<&str> = updates
        .iter()
        .filter_map(|update| update["offer"]["package"]["plugin_id"].as_str())
        .collect();
    assert_eq!(listed.len(), 2, "{offers}");
    assert!(
        listed.contains(&WIDER) && listed.contains(&MANUAL),
        "{offers}"
    );
    let widening = updates
        .iter()
        .find(|update| update["offer"]["package"]["plugin_id"] == WIDER)
        .expect("the widening update");
    assert_eq!(widening["adds_permissions"], true, "{widening}");
    assert_eq!(
        widening["added_permissions"],
        serde_json::json!({
            "granted": [],
            "http_domains": ["more.example.test"],
            "stream_hosts": []
        })
    );

    let (_, inventory) = get_json(&router, "/api/v1/plugins").await;
    let lifecycle = inventory["lifecycle"]
        .as_array()
        .expect("lifecycle")
        .iter()
        .find(|entry| entry["plugin_id"] == AUTOMATIC)
        .cloned()
        .expect("the updated plugin");
    assert_eq!(lifecycle["active_version"], "1.1.0", "{lifecycle}");
    assert_eq!(lifecycle["running_version"], "1.0.0", "{lifecycle}");
    assert_eq!(lifecycle["restart_required"], true, "{lifecycle}");
}

/// Crash and restart of an automatic update after its version folder exists (RD-180-12,
/// recovery matrix): the repository row and the version pointers are the two writes that follow
/// it, and a stop before either leaves the update installed and the choice as it was.
#[cfg(feature = "failpoints")]
mod stopped_updates {
    use super::*;

    /// Installs 1.0.0 of the automatic plugin with an explicit pointer at it, then runs the
    /// refresh that installs 1.1.0 with `point` armed.
    async fn update_stopped_at(directory: &std::path::Path, point: &str) -> common::Harness {
        let harness = common::test_harness(directory).await;
        let repository = rd_plugin_host::generate_signing_key();
        let author = rd_plugin_host::generate_signing_key();
        harness
            .state
            .plugins
            .verifier()
            .trust_key_base64("update-fixture-v1".to_owned(), &author.public_base64)
            .expect("trust the author");
        harness
            .state
            .plugins
            .install_bytes(update_fixture(
                &author,
                AUTOMATIC,
                "1.0.0",
                r#""example.test""#,
            ))
            .await
            .expect("install 1.0.0");
        harness
            .state
            .plugins
            .record_started_versions()
            .await
            .expect("the start records what it loads");
        // Pointed at 1.0.0, so an update that followed would move the pointer and one that did
        // not leaves it where it is: the difference is readable in the row.
        harness
            .database
            .save_plugin_version_choice(rd_db::NewPluginVersionChoice {
                plugin_id: AUTOMATIC.to_owned(),
                active_version: Some("1.0.0".to_owned()),
                previous_version: None,
                staged_version: None,
                update_policy: "automatic".to_owned(),
            })
            .await
            .expect("choice");

        let fetcher = std::sync::Arc::new(MapFetcher::default());
        let mut state = harness.state.clone();
        state.plugin_repositories =
            rd_plugin_host::repository::PluginRepositoryService::with_fetcher(
                harness.database.clone(),
                directory.join("plugin-repositories"),
                state.plugins.clone(),
                fetcher.clone(),
                None,
            );
        state
            .plugin_repositories
            .set_update_policy(std::sync::Arc::new(rd_api::VersionChoicePolicy::new(
                harness.database.clone(),
            )));
        let router = rd_api::router(state);
        fetcher.serve(GOOD_INDEX, signed_index(&repository, 1, &[]));
        add_repository(&router, GOOD_INDEX, &repository.public_base64).await;
        let update = update_fixture(&author, AUTOMATIC, "1.1.0", r#""example.test""#);
        fetcher.serve(&package_url(AUTOMATIC, "1.1.0"), update.clone());
        fetcher.serve(
            GOOD_INDEX,
            signed_index(&repository, 2, &[(AUTOMATIC, "1.1.0", update.as_slice())]),
        );

        let guard = rd_core::failpoint::FailpointGuard::once(point);
        let (status, body) = post_json(
            &router,
            "/api/v1/plugins/repositories/refresh",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(guard.fired(), "the crash point was never reached");
        harness
    }

    /// Both versions whole and listed once, no staging folder, the pointer still at 1.0.0 —
    /// and the start after the stop says so too.
    async fn assert_installed_and_unchosen(directory: &std::path::Path, harness: &common::Harness) {
        let mut versions: Vec<String> = harness
            .state
            .plugins
            .list_installed()
            .await
            .expect("list")
            .into_iter()
            .filter(|manifest| manifest.id.to_string() == AUTOMATIC)
            .map(|manifest| manifest.version)
            .collect();
        versions.sort();
        assert_eq!(versions, ["1.0.0", "1.1.0"]);
        let leftovers: Vec<String> = std::fs::read_dir(directory.join("plugins").join(AUTOMATIC))
            .expect("plugin folder")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let choice = harness
            .database
            .plugin_version_choice(AUTOMATIC)
            .await
            .expect("choice")
            .expect("a stored choice");
        assert_eq!(choice.active_version.as_deref(), Some("1.0.0"));
        assert_eq!(choice.previous_version, None);

        let restarted = common::test_harness(directory).await;
        let (status, inventory) = get_json(&restarted.router, "/api/v1/plugins").await;
        assert_eq!(status, StatusCode::OK, "{inventory}");
        let lifecycle = inventory["lifecycle"]
            .as_array()
            .expect("lifecycle")
            .iter()
            .find(|entry| entry["plugin_id"] == AUTOMATIC)
            .cloned()
            .expect("the plugin");
        assert_eq!(lifecycle["active_version"], "1.0.0", "{lifecycle}");
    }

    async fn recorded(harness: &common::Harness) -> bool {
        harness
            .database
            .list_plugin_repository_installs()
            .await
            .expect("installs")
            .iter()
            .any(|install| install.plugin_id == AUTOMATIC && install.version == "1.1.0")
    }

    /// `plugin.before_install_recorded`.
    #[tokio::test]
    async fn an_update_stopped_before_its_repository_row_stays_installed_and_unchosen() {
        let directory = tempfile::tempdir().expect("tempdir");
        let harness = update_stopped_at(directory.path(), "plugin.before_install_recorded").await;
        assert!(!recorded(&harness).await, "the row was written after all");
        assert_installed_and_unchosen(directory.path(), &harness).await;
    }

    /// `plugin.before_pointers_followed`.
    #[tokio::test]
    async fn an_update_stopped_before_its_pointers_followed_stays_installed_and_unchosen() {
        let directory = tempfile::tempdir().expect("tempdir");
        let harness = update_stopped_at(directory.path(), "plugin.before_pointers_followed").await;
        assert!(recorded(&harness).await, "the repository row is missing");
        assert_installed_and_unchosen(directory.path(), &harness).await;
    }
}
