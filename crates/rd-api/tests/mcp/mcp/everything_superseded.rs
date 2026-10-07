//! RD-1140-04: every superseded plugin version through one tool -- at the price of its route,
//! for one plugin by id or for every plugin, with the removal named in the answer.

use super::{
    API_BEARER, CONFIG_BEARER, NOBODY, envelope, handshake, installation, ok, refused_with,
};

const TOOL: &str = "remove_superseded_plugin_versions";
const PLUGIN: &str = "019d0000-0000-7000-8000-0000001140c4";

/// A loadable package for one version, as the admin suite's `plugin_superseded` writes one.
fn install_package(directory: &std::path::Path, version: &str) {
    let path = directory.join("plugins").join(PLUGIN).join(version);
    std::fs::create_dir_all(&path).expect("version directory");
    std::fs::write(
        path.join("manifest.toml"),
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.10.0"
id = "{PLUGIN}"
name = "Superseded tool"
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
slug = "superseded_tool"
kind = "hoster"
credentials = "api_key"
"#
        ),
    )
    .expect("manifest");
    std::fs::write(path.join("component.wasm"), b"\0asm\x0d\0\x01\0").expect("component");
}

fn exists(directory: &std::path::Path, version: &str) -> bool {
    directory
        .join("plugins")
        .join(PLUGIN)
        .join(version)
        .is_dir()
}

#[tokio::test]
async fn every_superseded_plugin_version_goes_through_one_tool() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = installation(directory.path()).await;
    install_package(directory.path(), "1.0.0");
    install_package(directory.path(), "2.0.0");

    // The price of the route: administration, which configuration does not include.
    let config = handshake(&router, CONFIG_BEARER).await;
    let answer = envelope(
        &router,
        CONFIG_BEARER,
        &config,
        TOOL,
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(
        answer["error"]["data"]["code"], "auth.scope_insufficient",
        "{answer}"
    );
    assert_eq!(answer["error"]["data"]["scope"], "api:admin", "{answer}");
    assert!(exists(directory.path(), "1.0.0"));

    let session = handshake(&router, API_BEARER).await;
    let code = refused_with(&router, &session, TOOL, serde_json::json!({ "id": NOBODY })).await;
    assert_eq!(code, "plugin.not_installed");

    let answer = ok(&router, &session, TOOL, serde_json::json!({ "id": PLUGIN })).await;
    assert_eq!(answer["code"], "plugin.superseded_removed", "{answer}");
    assert_eq!(answer["removed"][0]["version"], "1.0.0", "{answer}");
    assert_eq!(answer["kept"], serde_json::json!([]), "{answer}");
    assert!(!exists(directory.path(), "1.0.0"));
    assert!(
        exists(directory.path(), "2.0.0"),
        "the version that runs stays"
    );

    let answer = ok(&router, &session, TOOL, serde_json::json!({})).await;
    assert_eq!(answer["code"], "plugin.superseded_none", "{answer}");
}
