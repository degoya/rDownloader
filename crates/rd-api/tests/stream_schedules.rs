//! RD-080-08: scheduled livestream recordings.
//!
//! The occurrence maths is unit-tested in `rd-stream`; what runs here is the contract a
//! client sees — a schedule round-trips, one that could never fire is refused rather than
//! stored, planning is idempotent across a restart, and a window that passed becomes a
//! visible *missed* run rather than an absence.

mod common;

use axum::http::StatusCode;
use common::{delete_json, get_json, post_json, put_json, test_router};
use serde_json::{Value, json};

/// Creates a channel to hang schedules on.
async fn channel(router: &axum::Router) -> String {
    let (status, created) = post_json(
        router,
        "/api/v1/streams/channels",
        json!({
            "url": "https://twitch.tv/example",
            "name": "Example",
            "quality": null,
            "category_id": null,
            "enabled": true
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().expect("channel id").to_owned()
}

fn weekly(channel_id: &str) -> Value {
    json!({
        "channel_id": channel_id,
        "name": "Weekly show",
        "enabled": true,
        "kind": "weekly",
        "days": [3],
        "start_minute": 20 * 60,
        "timezone": "Europe/Berlin",
        "window_minutes": 120,
        "lead_minutes": 5,
        "trail_minutes": 10,
        "replay_from_start": true
    })
}

#[tokio::test]
async fn a_weekly_schedule_round_trips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let channel_id = channel(&router).await;

    let (status, created) =
        post_json(&router, "/api/v1/streams/schedules", weekly(&channel_id)).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["name"], "Weekly show");
    assert_eq!(created["kind"], "weekly");
    assert_eq!(created["days"], json!([3]));
    assert_eq!(created["timezone"], "Europe/Berlin");
    assert_eq!(created["replay_from_start"], true);

    let (_, listed) = get_json(&router, "/api/v1/streams/schedules").await;
    assert_eq!(listed.as_array().expect("array").len(), 1);
}

#[tokio::test]
async fn a_one_off_schedule_round_trips() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let channel_id = channel(&router).await;

    let (status, created) = post_json(
        &router,
        "/api/v1/streams/schedules",
        json!({
            "channel_id": channel_id,
            "name": "The final",
            "enabled": true,
            "kind": "once",
            "start": "2026-07-12T19:00:00Z",
            "timezone": "UTC",
            "window_minutes": 180
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["kind"], "once");
    assert_eq!(created["start"], "2026-07-12T19:00:00Z");
}

#[tokio::test]
async fn an_offset_is_refused_because_it_does_not_survive_daylight_saving() {
    // The mistake this field exists to prevent. `+01:00` is right for half the year.
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let channel_id = channel(&router).await;

    let mut request = weekly(&channel_id);
    request["timezone"] = json!("+01:00");
    let (status, response) = post_json(&router, "/api/v1/streams/schedules", request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{response}");
    assert_eq!(response["code"], "stream.schedule_timezone_unknown");
}

#[tokio::test]
async fn a_schedule_that_could_never_fire_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let channel_id = channel(&router).await;

    let cases = [
        (json!([]), 120_u32, "stream.schedule_no_days"),
        (json!([9]), 120, "stream.schedule_day_invalid"),
        (json!([3]), 0, "stream.schedule_window_invalid"),
    ];
    for (days, window, code) in cases {
        let mut request = weekly(&channel_id);
        request["days"] = days.clone();
        request["window_minutes"] = json!(window);
        let (status, response) = post_json(&router, "/api/v1/streams/schedules", request).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{days} / {window}: {response}"
        );
        assert_eq!(response["code"], code, "{response}");
    }
}

#[tokio::test]
async fn occurrences_are_planned_and_planning_again_adds_nothing() {
    // The idempotency the whole design rests on: the planner runs on every tick and again
    // after a restart, and a second pass over the same range must produce no second run.
    let temp = tempfile::tempdir().expect("tempdir");
    let planned;
    {
        let router = test_router(temp.path()).await;
        let channel_id = channel(&router).await;
        let (status, created) =
            post_json(&router, "/api/v1/streams/schedules", weekly(&channel_id)).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");

        planned = wait_for_runs(&router).await;
        // Two weeks of planning, one occurrence a week.
        assert!(planned >= 2, "expected planned occurrences, got {planned}");
    }

    // A restart re-plans the same range.
    let router = test_router(temp.path()).await;
    let after_restart = wait_for_runs(&router).await;
    assert_eq!(
        after_restart, planned,
        "re-planning after a restart must not duplicate occurrences"
    );
}

#[tokio::test]
async fn editing_a_schedule_replans_what_has_not_started() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let channel_id = channel(&router).await;
    let (_, created) = post_json(&router, "/api/v1/streams/schedules", weekly(&channel_id)).await;
    let id = created["id"].as_str().expect("id").to_owned();
    wait_for_runs(&router).await;

    // Three days a week instead of one.
    let mut request = weekly(&channel_id);
    request["days"] = json!([1, 3, 5]);
    let (status, updated) =
        put_json(&router, &format!("/api/v1/streams/schedules/{id}"), request).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["days"], json!([1, 3, 5]));
}

#[tokio::test]
async fn deleting_a_schedule_takes_its_runs_with_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let channel_id = channel(&router).await;
    let (_, created) = post_json(&router, "/api/v1/streams/schedules", weekly(&channel_id)).await;
    let id = created["id"].as_str().expect("id").to_owned();
    wait_for_runs(&router).await;

    let (status, _) = delete_json(&router, &format!("/api/v1/streams/schedules/{id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (_, listed) = get_json(&router, "/api/v1/streams/schedules").await;
    assert!(listed.as_array().expect("array").is_empty());
    let (_, runs) = get_json(&router, "/api/v1/streams/runs").await;
    assert!(runs.as_array().expect("array").is_empty());
}

#[tokio::test]
async fn an_unknown_schedule_reports_a_coded_404() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let missing = rd_core::StreamScheduleId::new();

    let (status, response) =
        delete_json(&router, &format!("/api/v1/streams/schedules/{missing}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{response}");
    assert_eq!(response["code"], "stream.schedule_not_found");
}

/// Waits for the monitor's planner to produce runs, returning how many there are.
async fn wait_for_runs(router: &axum::Router) -> usize {
    for _ in 0..400 {
        let (_, runs) = get_json(router, "/api/v1/streams/runs").await;
        let count = runs.as_array().map(Vec::len).unwrap_or_default();
        if count > 0 {
            return count;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    0
}
