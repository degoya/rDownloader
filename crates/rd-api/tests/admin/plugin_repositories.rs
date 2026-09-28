//! The plugin repository routes and the install preview (RD-140-01).
//!
//! Fetching, verifying, caching and withdrawing are tested in `rd_plugin_host::repository`
//! against an in-memory fetcher; these tests hold the REST contract that needs no network: the
//! built-in repository, disable without deletion, the refresh interval, the refusals that come
//! before any fetch, and a preview that reports a package's permissions without installing it.

use crate::common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{delete_json, get_json, patch_json, post_json, put_json, send, test_router};

const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

fn manifest(public_key: &str) -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000140aa"
name = "Preview Fixture"
version = "1.0.0"
key_id = "preview-fixture-v1"
public_key = "{public_key}"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A package that is only ever previewed"
author = "Fixture Author"

[provider]
slug = "previewfixture"
kind = "hoster"
credentials = "api_key"
"#
    )
}

async fn post_bytes(
    router: &axum::Router,
    uri: &str,
    bytes: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:8710")
        .header("content-type", "application/octet-stream")
        .body(Body::from(bytes))
        .expect("request");
    send(router, request).await
}

#[tokio::test]
async fn the_official_repository_is_built_in_and_can_be_switched_off_but_not_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = get_json(&router, "/api/v1/plugins/repositories").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let official = &body["repositories"][0];
    assert_eq!(official["id"], "official");
    assert_eq!(official["kind"], "official");
    assert_eq!(official["enabled"], true);
    assert_eq!(
        official["url"],
        rd_plugin_host::repository::OFFICIAL_INDEX_URL
    );
    assert_eq!(body["refresh_hours"], 24);

    let (status, body) = delete_json(&router, "/api/v1/plugins/repositories/official").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "plugin_repository.official_permanent");

    let (status, body) = patch_json(
        &router,
        "/api/v1/plugins/repositories/official",
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Switched off, a refresh leaves it alone: nothing is fetched and nothing is recorded.
    let (status, body) = post_json(
        &router,
        "/api/v1/plugins/repositories/refresh",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let official = &body["repositories"][0];
    assert_eq!(official["enabled"], false);
    assert!(official["last_checked_at"].is_null(), "{official}");

    let (status, body) = patch_json(
        &router,
        "/api/v1/plugins/repositories/nothing-like-this",
        serde_json::json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin_repository.not_found");
}

#[tokio::test]
async fn the_refresh_interval_is_bounded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    for hours in [0, 169] {
        let (status, body) = put_json(
            &router,
            "/api/v1/plugins/repositories/settings",
            serde_json::json!({ "refresh_hours": hours }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{hours}: {body}");
        assert_eq!(body["code"], "plugin_repository.refresh_hours_invalid");
    }
    let (status, body) = put_json(
        &router,
        "/api/v1/plugins/repositories/settings",
        serde_json::json!({ "refresh_hours": 6 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["refresh_hours"], 6);
}

/// Both refusals come before anything is fetched, so they hold without a network.
#[tokio::test]
async fn a_repository_needs_an_https_address_and_a_real_key() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let key = rd_plugin_host::generate_signing_key().public_base64;
    for (url, public_key, code) in [
        (
            "http://plugins.example.test/index.json",
            key.as_str(),
            "plugin_repository.url_invalid",
        ),
        (
            "https://user:pw@plugins.example.test/index.json",
            key.as_str(),
            "plugin_repository.url_invalid",
        ),
        (
            "https://plugins.example.test/index.json",
            "not a key",
            "plugin_repository.key_invalid",
        ),
    ] {
        let (status, body) = post_json(
            &router,
            "/api/v1/plugins/repositories",
            serde_json::json!({ "url": url, "public_key": public_key }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {body}");
        assert_eq!(body["code"], code, "{url}");
    }
    let (_, body) = get_json(&router, "/api/v1/plugins/repositories").await;
    assert_eq!(body["repositories"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn a_preview_shows_publisher_and_permissions_and_installs_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let key = rd_plugin_host::generate_signing_key();
    let package = rd_plugin_host::package_plugin(
        manifest(&key.public_base64).as_bytes(),
        EMPTY_COMPONENT,
        &[],
        Some(&key.signing_key),
    )
    .expect("package");

    let (status, preview) = post_bytes(&router, "/api/v1/plugins/preview", package).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["name"], "Preview Fixture");
    assert_eq!(preview["version"], "1.0.0");
    assert_eq!(preview["key_status"], "untrusted");
    assert_eq!(preview["publisher"]["key_id"], "preview-fixture-v1");
    assert_eq!(preview["publisher"]["author"], "Fixture Author");
    assert_eq!(
        preview["publisher"]["fingerprint"],
        rd_plugin_host::key_fingerprint(&key.signing_key.verifying_key())
    );
    assert_eq!(
        preview["permissions"]["granted"],
        serde_json::json!(["net_http"])
    );
    assert_eq!(
        preview["permissions"]["http_domains"],
        serde_json::json!(["example.test"])
    );
    assert_eq!(preview["installable"], true);
    assert_eq!(preview["installed_versions"], serde_json::json!([]));

    let (_, inventory) = get_json(&router, "/api/v1/plugins").await;
    assert_eq!(
        inventory["installed"].as_array().map(Vec::len),
        Some(0),
        "a preview installed something: {inventory}"
    );
    let (_, keys) = get_json(&router, "/api/v1/plugins/keys").await;
    assert_eq!(
        keys.as_array().map(Vec::len),
        Some(0),
        "a preview trusted a key"
    );

    let (status, body) =
        post_bytes(&router, "/api/v1/plugins/preview", b"not a zip".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "plugin.preview_failed");
}

#[tokio::test]
async fn without_a_loaded_index_there_are_no_updates_and_nothing_to_download() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, body) = get_json(&router, "/api/v1/plugins/updates").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        serde_json::json!({ "updates": [], "available": [], "installed": [] })
    );

    let (status, body) = post_json(
        &router,
        "/api/v1/plugins/repositories/official/install",
        serde_json::json!({ "plugin_id": "019d0000-0000-7000-8000-0000000140aa", "version": "1.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "plugin_repository.not_offered");
}
