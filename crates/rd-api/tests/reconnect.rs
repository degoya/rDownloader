//! The reconnect surface.
//!
//! The decision rules are unit-tested next to them; what is checked here is that the feature
//! stays inert until it is both switched on and configured, and that its settings are bounded.
//! Running an actual reconnect needs a router, so no test here ever starts one.

mod common;

use axum::http::StatusCode;
use common::{get_json, post_json, put_json, test_router};
use serde_json::json;

#[tokio::test]
async fn the_status_reports_an_idle_feature_that_is_switched_off() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = get_json(&router, "/api/v1/reconnect").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false, "reconnecting is never on unasked");
    assert_eq!(body["phase"], "idle");
    assert!(body["last"].is_null(), "nothing has been attempted: {body}");
    assert_eq!(body["blocked_hosts"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn asking_for_one_while_it_is_switched_off_says_so() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (status, body) = post_json(&router, "/api/v1/reconnect", json!({})).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "reconnect.disabled");
}

#[tokio::test]
async fn switching_it_on_without_a_script_is_refused_when_it_is_saved() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, settings) = get_json(&router, "/api/v1/settings").await;

    // A reconnect with nothing to run would hold the queue and then do nothing at all.
    let mut invalid = settings.clone();
    invalid["reconnect_enabled"] = json!(true);
    let (status, refused) = put_json(&router, "/api/v1/settings", invalid).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "settings.reconnect_script_missing");
}

#[tokio::test]
async fn the_interval_and_the_timeout_have_to_be_usable_spans() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, settings) = get_json(&router, "/api/v1/settings").await;

    for (field, value, code) in [
        (
            "reconnect_min_interval_minutes",
            0,
            "settings.reconnect_interval_invalid",
        ),
        (
            "reconnect_min_interval_minutes",
            1441,
            "settings.reconnect_interval_invalid",
        ),
        (
            "reconnect_timeout_seconds",
            29,
            "settings.reconnect_timeout_invalid",
        ),
        (
            "reconnect_timeout_seconds",
            901,
            "settings.reconnect_timeout_invalid",
        ),
    ] {
        let mut invalid = settings.clone();
        invalid[field] = json!(value);
        let (status, refused) = put_json(&router, "/api/v1/settings", invalid).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{field}={value}: {refused}"
        );
        assert_eq!(refused["code"], code, "{field}={value}");
    }
}

#[tokio::test]
async fn an_address_check_has_to_be_a_web_address() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, settings) = get_json(&router, "/api/v1/settings").await;

    let mut invalid = settings.clone();
    invalid["reconnect_ip_check_urls"] = json!(["file:///etc/passwd"]);
    let (status, refused) = put_json(&router, "/api/v1/settings", invalid).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "settings.reconnect_url_invalid");
}

#[tokio::test]
async fn a_configured_reconnect_is_accepted_and_reported_as_enabled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, settings) = get_json(&router, "/api/v1/settings").await;

    let mut configured = settings.clone();
    // A settings write needs the login either set up or deliberately switched off.
    configured["admin_login_disabled"] = json!(true);
    configured["reconnect_enabled"] = json!(true);
    configured["reconnect_script"] = json!("reconnect.sh");
    configured["reconnect_ip_check_urls"] = json!(["https://ip.example/plain"]);
    let (status, saved) = put_json(&router, "/api/v1/settings", configured).await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (_, body) = get_json(&router, "/api/v1/reconnect").await;
    assert_eq!(body["enabled"], true, "{body}");
    assert_eq!(body["phase"], "idle", "still nothing running: {body}");
}
