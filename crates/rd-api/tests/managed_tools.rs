//! The managed external tools over REST (RD-102-02).
//!
//! The store itself is covered in `crates/rd-tools/tests/managed_tools.rs`. What is checked
//! here is the API contract around it: the shape of the status document, the stable `tools.*`
//! codes, and the two refusals a user is most likely to hit — the feature being switched off,
//! and a manifest URL that is not https.

mod common;

use axum::http::StatusCode;
use common::{get_json, post_json, put_json, test_router};
use serde_json::json;

/// Reads the settings document, applies a patch, and writes it back.
async fn update(
    router: &axum::Router,
    patch: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let (_, mut settings) = get_json(router, "/api/v1/settings").await;
    settings["admin_login_disabled"] = serde_json::Value::Bool(true);
    for (key, value) in patch.as_object().expect("patch object") {
        settings[key] = value.clone();
    }
    put_json(router, "/api/v1/settings", settings).await
}

/// A fresh installation manages nothing and has downloaded nothing. Every managed tool is
/// listed all the same, so the interface can offer them rather than hide the feature.
#[tokio::test]
async fn a_fresh_installation_lists_every_managed_tool_with_nothing_installed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = get_json(&router, "/api/v1/system/tools").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false, "managed tools are opt-in");
    assert!(
        body["platform"]
            .as_str()
            .is_some_and(|triple| triple.contains('-')),
        "{body}"
    );

    let tools = body["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        ["yt-dlp", "gallery-dl", "streamlink", "ffmpeg", "ffprobe"],
        "{body}"
    );
    for tool in tools {
        assert!(tool["active_version"].is_null(), "{tool}");
        assert_eq!(tool["installed_versions"].as_array().map(Vec::len), Some(0));
        assert_eq!(tool["can_roll_back"], false, "{tool}");
    }
}

/// Switched off means switched off, and it says so with its own code rather than with a
/// generic failure the user cannot act on.
#[tokio::test]
async fn installing_while_managed_tools_are_switched_off_is_refused_with_its_own_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = post_json(
        &router,
        "/api/v1/system/tools/yt-dlp/install",
        json!({ "version": null }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "tools.disabled", "{body}");
}

/// A name outside the closed list is a bad request, not a 404 that reads like a typo in the
/// route.
#[tokio::test]
async fn a_tool_the_application_does_not_manage_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    update(&router, json!({ "managed_tools_enabled": true })).await;

    let (status, body) = post_json(
        &router,
        "/api/v1/system/tools/curl/install",
        json!({ "version": null }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "tools.not_managed", "{body}");
}

/// Activating a version nothing installed must not leave the pointer aiming at nothing, and
/// the code says which of the several possible problems it was.
#[tokio::test]
async fn activating_an_uninstalled_version_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = post_json(
        &router,
        "/api/v1/system/tools/yt-dlp/activate",
        json!({ "version": "2099.01.01" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "tools.version_not_installed", "{body}");
}

/// Rolling back with one version or none is refused rather than silently doing nothing.
#[tokio::test]
async fn rolling_back_with_nothing_to_return_to_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) =
        post_json(&router, "/api/v1/system/tools/yt-dlp/rollback", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "tools.nothing_to_roll_back_to", "{body}");
}

/// The setting is refused at the point it is saved, naming the value — not later, with a
/// signature error on a document nobody tampered with.
#[tokio::test]
async fn a_manifest_url_that_is_not_https_is_refused_when_it_is_saved() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = update(
        &router,
        json!({ "managed_tools_manifest_url": "http://example.invalid/tools.json" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["code"], "settings.managed_tools_manifest_url_invalid",
        "{body}"
    );

    let (status, body) = update(
        &router,
        json!({ "managed_tools_manifest_url": "https://example.invalid/tools.json" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The tool-status document gains the two managed fields without losing what it had: a system
/// binary still reports its path and source, and `managed` says only whether the application
/// *could* manage that tool.
#[tokio::test]
async fn the_tool_status_document_reports_what_is_managed_without_claiming_a_version() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = get_json(&router, "/api/v1/system/media").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (key, managed) in [
        ("ytdlp", true),
        ("ffmpeg", true),
        ("ffprobe", true),
        ("gallery_dl", true),
        ("streamlink", true),
        ("unrar", false),
        ("seven_zip", false),
        ("rclone", false),
        ("apprise", false),
    ] {
        assert_eq!(body[key]["managed"], managed, "{key}: {body}");
        assert!(
            body[key]["active_version"].is_null(),
            "{key} must not claim a managed version: {body}"
        );
    }
}

/// RD-102-03: the status document carries a verdict per tool, and the four states are
/// distinguishable to a client rather than collapsed into "ok"/"not ok".
#[tokio::test]
async fn the_media_status_document_carries_a_compatibility_verdict_per_tool() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = get_json(&router, "/api/v1/system/media").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for name in [
        "ytdlp",
        "ffmpeg",
        "ffprobe",
        "unrar",
        "seven_zip",
        "rclone",
        "gallery_dl",
        "streamlink",
        "apprise",
    ] {
        let compatibility = &body[name]["compatibility"];
        let verdict = compatibility["verdict"].as_str().unwrap_or_default();
        assert!(
            ["supported", "too_old", "known_bad", "unknown"].contains(&verdict),
            "{name}: {compatibility}"
        );
        assert_eq!(
            compatibility["overridden"], false,
            "{name}: {compatibility}"
        );
        assert!(
            compatibility["affects"].is_array(),
            "{name}: {compatibility}"
        );
    }
    // A tool no rule covers reports `unknown` and gates nothing, rather than being called
    // supported on no evidence.
    assert_eq!(
        body["unrar"]["compatibility"]["verdict"], "unknown",
        "{body}"
    );
    assert_eq!(
        body["unrar"]["compatibility"]["affects"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "{body}"
    );
}

/// RD-102-03: an override is a decision about a named tool. A name with no rule behind it is
/// refused rather than stored, because it would look like it did something and do nothing.
#[tokio::test]
async fn a_compatibility_override_must_name_a_tool_that_has_a_rule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = update(&router, json!({ "tool_compatibility_overrides": ["curl"] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["code"], "settings.tool_compatibility_override_invalid",
        "{body}"
    );

    let (status, body) = update(
        &router,
        json!({ "tool_compatibility_overrides": [" YT-DLP ", "yt-dlp"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["tool_compatibility_overrides"],
        json!(["yt-dlp"]),
        "the list is normalised and de-duplicated: {body}"
    );
}
