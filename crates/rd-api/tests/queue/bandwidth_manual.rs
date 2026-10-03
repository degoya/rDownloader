//! Switching a bandwidth profile on by hand, in front of the schedule (RD-190-20): which
//! profile is active and why, its end, switching back, and what is refused.

use crate::common;

use axum::{Router, http::StatusCode};

async fn profile(router: &Router, name: &str) -> String {
    let (status, created) = common::post_json(
        router,
        "/api/v1/bandwidth/profiles",
        serde_json::json!({ "name": name, "download_bytes_per_second": "1000000" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("profile id").to_owned()
}

/// Two profiles, the first applying whenever no window does.
async fn scheduled_day_and_spare_night(router: &Router) -> (String, String) {
    let day = profile(router, "Day").await;
    let night = profile(router, "Night").await;
    let (status, schedule) = common::put_json(
        router,
        "/api/v1/bandwidth/schedule",
        serde_json::json!({ "timezone": "UTC", "default_profile_id": day, "windows": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{schedule}");
    (day, night)
}

#[tokio::test]
async fn a_profile_switched_on_by_hand_holds_until_it_is_switched_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (day, night) = scheduled_day_and_spare_night(&router).await;

    let (_, status) = common::get_json(&router, "/api/v1/bandwidth/status").await;
    assert_eq!(status["active_profile"]["id"], day.as_str(), "{status}");
    assert_eq!(status["source"], "schedule");
    assert!(status["manual"].is_null(), "{status}");

    let (code, switched) = common::put_json(
        &router,
        "/api/v1/bandwidth/manual",
        serde_json::json!({ "profile_id": night, "ends": "never" }),
    )
    .await;
    assert_eq!(code, StatusCode::OK, "{switched}");
    assert_eq!(
        switched["active_profile"]["id"],
        night.as_str(),
        "{switched}"
    );
    assert_eq!(switched["source"], "manual");
    assert_eq!(switched["manual"]["ends"], "never");
    assert!(switched["manual"]["until"].is_null(), "{switched}");
    assert!(switched["next_switch_at"].is_null(), "{switched}");

    let (code, back) = common::delete_json(&router, "/api/v1/bandwidth/manual").await;
    assert_eq!(code, StatusCode::OK, "{back}");
    assert_eq!(back["active_profile"]["id"], day.as_str(), "{back}");
    assert_eq!(back["source"], "schedule");
}

#[tokio::test]
async fn a_chosen_end_is_the_next_switch_and_no_limits_is_a_choice() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    scheduled_day_and_spare_night(&router).await;
    let until = (chrono::Utc::now() + chrono::Duration::hours(2))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let (code, switched) = common::put_json(
        &router,
        "/api/v1/bandwidth/manual",
        serde_json::json!({ "ends": "at", "until": until }),
    )
    .await;

    assert_eq!(code, StatusCode::OK, "{switched}");
    assert!(
        switched["active_profile"].is_null(),
        "no limits: {switched}"
    );
    assert_eq!(switched["source"], "manual");
    let asked: chrono::DateTime<chrono::Utc> = until.parse().expect("time");
    let next: chrono::DateTime<chrono::Utc> = switched["next_switch_at"]
        .as_str()
        .expect("next switch")
        .parse()
        .expect("time");
    assert_eq!(next, asked, "{switched}");
}

#[tokio::test]
async fn deleting_the_profile_in_force_hands_back_to_the_schedule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (day, night) = scheduled_day_and_spare_night(&router).await;
    let (code, _) = common::put_json(
        &router,
        "/api/v1/bandwidth/manual",
        serde_json::json!({ "profile_id": night, "ends": "never" }),
    )
    .await;
    assert_eq!(code, StatusCode::OK);

    let (code, deleted) =
        common::delete_json(&router, &format!("/api/v1/bandwidth/profiles/{night}")).await;
    assert_eq!(code, StatusCode::OK, "{deleted}");

    let (_, status) = common::get_json(&router, "/api/v1/bandwidth/status").await;
    assert_eq!(status["active_profile"]["id"], day.as_str(), "{status}");
    assert_eq!(status["source"], "schedule");
}

#[tokio::test]
async fn an_unknown_profile_or_a_bad_end_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (_, night) = scheduled_day_and_spare_night(&router).await;
    let past = (chrono::Utc::now() - chrono::Duration::minutes(1)).to_rfc3339();
    let far = (chrono::Utc::now() + chrono::Duration::days(31)).to_rfc3339();
    for (body, code) in [
        (
            serde_json::json!({
                "profile_id": "0192f0c4-0000-7000-8000-00000000abcd",
                "ends": "never"
            }),
            "bandwidth.profile_not_found",
        ),
        (
            serde_json::json!({ "profile_id": night, "ends": "at" }),
            "bandwidth.manual_end_invalid",
        ),
        (
            serde_json::json!({ "profile_id": night, "ends": "at", "until": past }),
            "bandwidth.manual_end_invalid",
        ),
        (
            serde_json::json!({ "profile_id": night, "ends": "at", "until": far }),
            "bandwidth.manual_end_invalid",
        ),
    ] {
        let (status, refused) =
            common::put_json(&router, "/api/v1/bandwidth/manual", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {refused}");
        assert_eq!(refused["code"], code, "{body}: {refused}");
    }
    let (_, status) = common::get_json(&router, "/api/v1/bandwidth/status").await;
    assert_eq!(
        status["source"], "schedule",
        "a refused switch must not hold: {status}"
    );
}
