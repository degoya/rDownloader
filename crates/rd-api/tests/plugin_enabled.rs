//! Switching an installed plugin off and back on.
//!
//! The flag lives in the settings document rather than in a table of its own, next to the
//! post-processing step list that already works that way. What matters here is the contract a
//! client sees: the state persists, it is idempotent, and a disabled plugin is still listed —
//! otherwise there would be no way to switch it back on.

mod common;

use axum::http::StatusCode;
use common::{get_json, patch_json, test_router};

const PLUGIN: &str = "019d0000-0000-7000-8000-000000000107";

#[tokio::test]
async fn a_plugin_can_be_switched_off_and_on_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    assert_eq!(
        settings["disabled_plugins"].as_array().map(Vec::len),
        Some(0),
        "nothing is switched off to begin with"
    );

    let (status, body) = patch_json(
        &router,
        &format!("/api/v1/plugins/{PLUGIN}"),
        serde_json::json!({ "enabled": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    assert_eq!(
        settings["disabled_plugins"],
        serde_json::json!([PLUGIN]),
        "the choice is persisted in the settings document"
    );

    let (status, _) = patch_json(
        &router,
        &format!("/api/v1/plugins/{PLUGIN}"),
        serde_json::json!({ "enabled": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    assert_eq!(
        settings["disabled_plugins"],
        serde_json::json!([]),
        "switching it on again removes it rather than leaving a stale entry"
    );
}

#[tokio::test]
async fn switching_the_same_plugin_off_twice_records_it_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    for _ in 0..2 {
        let (status, body) = patch_json(
            &router,
            &format!("/api/v1/plugins/{PLUGIN}"),
            serde_json::json!({ "enabled": false }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    assert_eq!(settings["disabled_plugins"], serde_json::json!([PLUGIN]));
}
