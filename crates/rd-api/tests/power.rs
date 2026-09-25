//! Integration tests for the quiet-hours and completion-action surface (RD-050-13).

mod common;

use axum::http::StatusCode;

#[tokio::test]
async fn the_status_reports_the_platform_capabilities_and_the_quiet_state() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, power) = common::get_json(&router, "/api/v1/power/status").await;
    assert_eq!(status, StatusCode::OK, "{power}");
    // Whatever the platform can do, the matrix has to state it either way.
    assert!(power["capabilities"]["standby"].is_boolean());
    assert!(power["capabilities"]["shutdown"].is_boolean());
    assert_eq!(power["action"], "none");
    assert_eq!(power["quiet"], false);
    assert!(power["pending"].is_null());
}

#[tokio::test]
async fn cancelling_without_a_pending_action_says_so_instead_of_failing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, cancelled) =
        common::post_json(&router, "/api/v1/power/cancel", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["code"], "power.nothing_pending");
}

#[tokio::test]
async fn a_power_action_without_a_countdown_or_a_script_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, settings) = common::get_json(&router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");

    let mut invalid = settings.clone();
    invalid["admin_login_disabled"] = serde_json::json!(true);
    invalid["completion_countdown_seconds"] = serde_json::json!(1);
    let (status, refused) = common::put_json(&router, "/api/v1/settings", invalid).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "power.countdown_invalid");

    let mut invalid = settings.clone();
    invalid["admin_login_disabled"] = serde_json::json!(true);
    invalid["completion_action"] = serde_json::json!("script");
    let (status, refused) = common::put_json(&router, "/api/v1/settings", invalid).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "power.script_missing");
}

#[tokio::test]
async fn quiet_hours_are_reported_once_a_window_covers_the_current_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    // A window covering the whole week is quiet whenever the test runs.
    harness
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({
                "bandwidth_timezone": "UTC",
                "quiet_hours": {
                    "enabled": true,
                    "windows": [{ "days": 0b0111_1111, "start_minute": 0, "end_minute": 1440 }]
                }
            }),
        )
        .await
        .expect("settings");

    for _ in 0..60 {
        let (_, power) = common::get_json(&harness.router, "/api/v1/power/status").await;
        if power["quiet"] == true {
            // A window covering every minute of the week never ends, so there is no end to
            // report — the field stays empty rather than inventing one.
            assert!(power["quiet_until"].is_null(), "{power}");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    panic!("quiet hours were never reported");
}
