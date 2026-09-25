//! Integration tests for the bandwidth profiles, their schedule and the live status
//! (RD-050-12).

mod common;

use axum::{Router, http::StatusCode};

const EVERY_DAY: u8 = 0b0111_1111;

async fn create_profile(router: &Router, name: &str, body: serde_json::Value) -> String {
    let mut payload = serde_json::json!({ "name": name });
    if let (Some(target), Some(extra)) = (payload.as_object_mut(), body.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    let (status, profile) = common::post_json(router, "/api/v1/bandwidth/profiles", payload).await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    profile["id"].as_str().expect("profile id").to_owned()
}

#[tokio::test]
async fn a_profile_round_trips_with_its_scope_limits() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let id = create_profile(
        &router,
        "Night",
        serde_json::json!({
            "download_bytes_per_second": "20000000",
            "max_active_files": 6,
            "daily_budget_bytes": "50000000000",
            "scopes": [
                { "kind": "protocol", "value": "usenet", "bytes_per_second": 5_000_000u64 },
                { "kind": "host", "value": "example.com", "bytes_per_second": 1_000_000u64 }
            ]
        }),
    )
    .await;

    let (status, profiles) = common::get_json(&router, "/api/v1/bandwidth/profiles").await;
    assert_eq!(status, StatusCode::OK, "{profiles}");
    let stored = profiles.as_array().expect("profiles");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["name"], "Night");
    assert_eq!(stored[0]["download_bytes_per_second"], "20000000");
    assert_eq!(stored[0]["scopes"].as_array().expect("scopes").len(), 2);

    let (status, deleted) =
        common::delete_json(&router, &format!("/api/v1/bandwidth/profiles/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    let (_, profiles) = common::get_json(&router, "/api/v1/bandwidth/profiles").await;
    assert!(profiles.as_array().expect("profiles").is_empty());
}

#[tokio::test]
async fn a_schedule_activates_its_profile_and_reports_the_next_switch() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let night = create_profile(
        &router,
        "Night",
        serde_json::json!({ "download_bytes_per_second": "1000000" }),
    )
    .await;

    // A window covering the whole week means the profile is active whenever the test runs,
    // which keeps the assertion independent of the clock.
    let (status, schedule) = common::put_json(
        &router,
        "/api/v1/bandwidth/schedule",
        serde_json::json!({
            "timezone": "Europe/Berlin",
            "windows": [{
                "profile_id": night,
                "days": EVERY_DAY,
                "start_minute": 0,
                "end_minute": 1440,
                "priority": 0,
                "enabled": true
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{schedule}");
    assert_eq!(schedule["timezone"], "Europe/Berlin");

    let (status, live) = common::get_json(&router, "/api/v1/bandwidth/status").await;
    assert_eq!(status, StatusCode::OK, "{live}");
    assert_eq!(live["active_profile"]["id"], night);
    assert_eq!(live["timezone"], "Europe/Berlin");
    assert_eq!(live["binding_limit"]["bytes_per_second"], "1000000");
    assert_eq!(live["binding_limit"]["source"], "global");
    assert_eq!(live["budget_exhausted"], false);
}

#[tokio::test]
async fn the_hand_set_speed_limit_wins_when_it_is_stricter_than_the_profile() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = harness.router.clone();
    let profile = create_profile(
        &router,
        "Day",
        serde_json::json!({ "download_bytes_per_second": "5000000" }),
    )
    .await;
    let (status, saved) = common::put_json(
        &router,
        "/api/v1/bandwidth/schedule",
        serde_json::json!({
            "timezone": "UTC",
            "default_profile_id": profile,
            "windows": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    // The toolbar limit is independent of the schedule and applies on top of it.
    harness
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "speed_limit_bytes_per_second": "250000" }),
        )
        .await
        .expect("settings");
    let (status, settings) = common::get_json(&router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    let mut settings = settings;
    // The harness stands in for an installation without an admin password, and saving the
    // settings re-applies that switch.
    settings["admin_login_disabled"] = serde_json::json!(true);
    let (status, applied) = common::put_json(&router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{applied}");

    let (_, live) = common::get_json(&router, "/api/v1/bandwidth/status").await;
    assert_eq!(
        live["binding_limit"]["bytes_per_second"], "250000",
        "{live}"
    );
    assert_eq!(live["binding_limit"]["source"], "manual");
}

#[tokio::test]
async fn a_schedule_referencing_an_unknown_profile_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, refused) = common::put_json(
        &router,
        "/api/v1/bandwidth/schedule",
        serde_json::json!({
            "timezone": "UTC",
            "windows": [{
                "profile_id": rd_core::BandwidthProfileId::new(),
                "days": EVERY_DAY,
                "start_minute": 0,
                "end_minute": 60
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "bandwidth.profile_not_found");

    let (status, refused) = common::put_json(
        &router,
        "/api/v1/bandwidth/schedule",
        serde_json::json!({ "timezone": "Mars/Olympus", "windows": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "bandwidth.timezone_invalid");
}

#[tokio::test]
async fn the_capability_matrix_names_what_a_limit_cannot_reach() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, capabilities) = common::get_json(&router, "/api/v1/bandwidth/capabilities").await;
    assert_eq!(status, StatusCode::OK, "{capabilities}");
    let entries = capabilities.as_array().expect("capabilities");
    let record = entries
        .iter()
        .find(|entry| entry["kind"] == "record")
        .expect("record entry");
    // A live recording cannot be slowed down, and the matrix says so instead of pretending.
    assert_eq!(record["download_enforced"], false);
    assert!(record["note"].as_str().is_some_and(|note| !note.is_empty()));
    let http = entries
        .iter()
        .find(|entry| entry["kind"] == "http")
        .expect("http entry");
    assert_eq!(http["download_enforced"], true);
    assert_eq!(http["scoped_enforced"], true);
}

/// Puts `profile` (or none) in charge through the schedule endpoint, which reloads the
/// bandwidth state, and answers whether new transfers are held back for the budget.
async fn activate(router: &Router, profile: Option<&str>) -> bool {
    let (status, saved) = common::put_json(
        router,
        "/api/v1/bandwidth/schedule",
        serde_json::json!({ "timezone": "UTC", "default_profile_id": profile, "windows": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, live) = common::get_json(router, "/api/v1/bandwidth/status").await;
    assert_eq!(status, StatusCode::OK, "{live}");
    live["budget_exhausted"]
        .as_bool()
        .unwrap_or_else(|| panic!("no budget_exhausted in {live}"))
}

/// RD-120-64: a used-up budget belongs to its profile. It ends when no profile is active, a
/// switch does not carry it over to another one, and `budget_exhausted` goes out once for the
/// profile and the day, however often the profile comes and goes.
#[tokio::test]
async fn a_used_up_budget_ends_with_its_profile_and_is_announced_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = harness.router.clone();
    let spent = create_profile(
        &router,
        "Capped",
        serde_json::json!({ "daily_budget_bytes": "500" }),
    )
    .await;
    let other = create_profile(
        &router,
        "Other",
        serde_json::json!({ "daily_budget_bytes": "500" }),
    )
    .await;
    // Today's counter of the first profile is past its limit; the schedule runs in UTC.
    let now = chrono::Utc::now();
    let period = |key: String| rd_limits::BudgetPeriod {
        key,
        used_bytes: 600,
    };
    harness
        .database
        .store_bandwidth_budget(
            serde_json::from_value(serde_json::json!(spent)).expect("profile id"),
            rd_limits::BudgetState {
                daily: period(now.format("%Y-%m-%d").to_string()),
                monthly: period(now.format("%Y-%m").to_string()),
                last_total_bytes: 0,
            },
        )
        .await
        .expect("store the budget");
    let mut events = harness.database.subscribe();

    assert!(
        activate(&router, Some(&spent)).await,
        "the capped profile is used up"
    );
    // No profile, no budget: new transfers may start again.
    assert!(
        !activate(&router, None).await,
        "the budget outlived its profile"
    );
    assert!(activate(&router, Some(&spent)).await);
    // Another profile does not inherit the verdict.
    assert!(
        !activate(&router, Some(&other)).await,
        "the verdict was carried over"
    );

    let mut announced = 0;
    while let Ok(event) = events.try_recv() {
        if event.kind == rd_core::EventKind::BandwidthChanged
            && event.payload["entity"] == "budget"
            && event.payload["exhausted"] == true
        {
            announced += 1;
        }
    }
    assert_eq!(
        announced, 1,
        "budget_exhausted is announced once per profile and day"
    );
}
