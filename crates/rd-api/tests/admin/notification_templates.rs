//! A rule's own wording (RD-1280-02): the rule route keeps a title and a text template, refuses
//! one naming no placeholder with a stable code, hands out the placeholder table, and a finished
//! package reaches the webhook in the rule's words -- the JSON keeping its fields -- with the
//! active traffic budget's figures of the moment (RD-1290-02), and a budget counted from the
//! first sample runs out and reaches its rule (RD-1291-02). Bursts take the wording too
//! (RD-1291-01, `notification_activity`).

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use axum::http::StatusCode;
use rd_core::{PackageId, PackageState};
use serde_json::json;

async fn rule(
    router: &axum::Router,
    target: &serde_json::Value,
    extra: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut body = json!({
        "name": "own words",
        "target_id": target["id"],
        "events": ["package_completed"]
    });
    for (key, value) in extra.as_object().expect("an object") {
        body[key] = value.clone();
    }
    common::post_json(router, "/api/v1/notifications/rules", body).await
}

#[tokio::test]
async fn a_rule_keeps_its_wording_and_refuses_a_placeholder_that_does_not_exist() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let target = create_target(
        &router,
        json!({ "name": "hook", "kind": "webhook", "endpoint": "https://hooks.example.com/rd" }),
    )
    .await;

    let (status, saved) = rule(
        &router,
        &target,
        json!({ "title_template": "{package} fertig um {time}", "body_template": "  " }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["title_template"], "{package} fertig um {time}");
    // A blank template is none: the built-in text goes out.
    assert!(saved["body_template"].is_null(), "{saved}");
    let (status, listed) = common::get_json(&router, "/api/v1/notifications/rules").await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed[0]["title_template"], "{package} fertig um {time}");

    let refusals = [
        (
            json!({ "title_template": "Done: {pakage}" }),
            "notification.template_unknown_placeholder",
            "title_template",
        ),
        (
            json!({ "body_template": "{package" }),
            "notification.template_syntax",
            "body_template",
        ),
        (
            json!({ "title_template": "x".repeat(201) }),
            "notification.template_too_long",
            "title_template",
        ),
    ];
    for (extra, code, field) in refusals {
        let (status, refused) = rule(&router, &target, extra).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["code"], code, "{refused}");
        assert_eq!(refused["params"]["field"], field, "{refused}");
    }
    let (_, refused) = rule(&router, &target, json!({ "title_template": "{pakage}" })).await;
    assert_eq!(refused["params"]["placeholder"], "pakage", "{refused}");
    let (_, refused) = rule(
        &router,
        &target,
        json!({ "title_template": "x".repeat(201) }),
    )
    .await;
    assert_eq!(refused["params"]["max"], "200", "{refused}");

    // An update without the fields returns the rule to the built-in wording.
    let id = saved["id"].as_str().expect("id");
    let (status, updated) = common::put_json(
        &router,
        &format!("/api/v1/notifications/rules/{id}"),
        json!({ "name": "own words", "target_id": target["id"], "events": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert!(updated["title_template"].is_null(), "{updated}");
}

#[tokio::test]
async fn the_placeholder_table_names_what_each_event_fills() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, table) = common::get_json(&router, "/api/v1/notifications/placeholders").await;
    assert_eq!(status, StatusCode::OK, "{table}");
    let names: Vec<&str> = table["placeholders"]
        .as_array()
        .expect("placeholders")
        .iter()
        .map(|entry| entry["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names.len(), rd_notify::Placeholder::ALL.len());
    for wanted in ["package", "time", "date", "storage_target", "error"] {
        assert!(names.contains(&wanted), "{wanted}: {names:?}");
    }
    let event = |code: &str| {
        table["events"]
            .as_array()
            .expect("events")
            .iter()
            .find(|entry| entry["event"] == code)
            .cloned()
            .expect("listed")
    };
    let finished = event("package_completed");
    assert_eq!(finished["takes_template"], true);
    assert_eq!(
        finished["placeholders"],
        json!([
            "event",
            "severity",
            "time",
            "date",
            "title",
            "message",
            "budget_used",
            "budget_left",
            "budget_limit",
            "budget_period",
            "package",
            "category",
            "state",
            "size",
            "path"
        ])
    );
    assert_eq!(finished["in_bursts"], false);
    assert_eq!(finished["examples"], json!([]));
    // RD-1291-01: a burst takes the rule's wording, and the preview a sample burst.
    let started = event("download_started");
    assert_eq!(started["takes_template"], true);
    assert_eq!(started["in_bursts"], true);
    let specific: Vec<&str> = started["placeholders"]
        .as_array()
        .expect("placeholders")
        .iter()
        .skip(rd_notify::Placeholder::COMMON.len())
        .map(|name| name.as_str().expect("a name"))
        .collect();
    assert_eq!(
        specific,
        ["count", "items", "names", "file", "package", "category"]
    );
    let sample = |entry: &serde_json::Value, name: &str| {
        entry["examples"]
            .as_array()
            .expect("examples")
            .iter()
            .find(|example| example["name"] == name)
            .map(|example| example["example"].clone())
    };
    assert_eq!(
        sample(&started, "title"),
        Some(json!("3 downloads started"))
    );
    assert_eq!(sample(&started, "file"), Some(json!("")), "{started}");
    let added = event("links_added");
    assert_eq!(added["takes_template"], true);
    assert_eq!(sample(&added, "items"), Some(json!("12")), "{added}");
    for wanted in ["count", "items", "names"] {
        assert!(names.contains(&wanted), "{wanted}: {names:?}");
    }
    assert!(
        table["events"]
            .as_array()
            .expect("events")
            .iter()
            .all(|entry| entry["event"] != "automation"),
        "no rule names an automation's message"
    );
}

#[tokio::test]
async fn a_finished_package_reaches_the_webhook_in_the_rule_s_words() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (_server, endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, saved) = rule(
        &router,
        &target,
        json!({
            "title_template": "{package} \u{2713} {{done}} at {time}",
            "body_template": "{message}\nSize: ({size}) Folder: {path}"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");

    let (status, created) = common::post_json(
        &router,
        "/api/v1/downloads",
        json!({ "url": "https://example.test/holiday.bin", "package_name": "Holiday" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id: PackageId = created["package_id"]
        .as_str()
        .expect("package id")
        .parse()
        .expect("package id");
    let folder = harness
        .database
        .get_package(id)
        .await
        .expect("package")
        .expect("exists")
        .destination;
    harness
        .database
        .set_package_state(id, PackageState::Completed, None, None, None)
        .await
        .expect("state");

    let received = &calls;
    common::eventually(
        std::time::Duration::from_secs(20),
        "the delivery did not arrive",
        || async move { (!received.lock().expect("calls").is_empty()).then_some(()) },
    )
    .await;
    let body = calls.lock().expect("calls")[0].2.clone();
    let payload: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    let mut keys: Vec<&String> = payload.as_object().expect("an object").keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["body", "event", "idempotency_key", "title"],
        "{payload}"
    );
    assert_eq!(payload["event"], "package_completed");
    let title = payload["title"].as_str().expect("title");
    let time = title
        .strip_prefix("Holiday \u{2713} {done} at ")
        .unwrap_or_else(|| panic!("the rule's title: {title}"));
    assert!(
        time.len() == 5 && time.as_bytes()[2] == b':',
        "HH:MM in the service's zone: {title}"
    );
    // `{size}` has no value for a package nothing was downloaded for: empty, not a guess.
    assert_eq!(
        payload["body"],
        format!("Holiday (completed)\nSize: () Folder: {folder}"),
        "{payload}"
    );
}

/// RD-1290-02: the budget placeholders carry the active profile's figures when the delivery is
/// queued.
#[tokio::test]
async fn a_finished_package_names_the_traffic_budget_left() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (status, profile) = common::post_json(
        &router,
        "/api/v1/bandwidth/profiles",
        json!({ "name": "Capped", "daily_budget_bytes": "1073741824" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    let (status, schedule) = common::put_json(
        &router,
        "/api/v1/bandwidth/schedule",
        json!({ "timezone": "Europe/Berlin", "default_profile_id": profile["id"], "windows": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{schedule}");
    let (_server, endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, saved) = rule(
        &router,
        &target,
        json!({ "body_template": "{budget_used} used, {budget_left} of {budget_limit} left ({budget_period})" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");

    let (status, created) = common::post_json(
        &router,
        "/api/v1/downloads",
        json!({ "url": "https://example.test/budget.bin", "package_name": "Budget" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id: PackageId = created["package_id"]
        .as_str()
        .expect("package id")
        .parse()
        .expect("package id");
    harness
        .database
        .set_package_state(id, PackageState::Completed, None, None, None)
        .await
        .expect("state");

    let received = &calls;
    common::eventually(
        std::time::Duration::from_secs(20),
        "the delivery did not arrive",
        || async move { (!received.lock().expect("calls").is_empty()).then_some(()) },
    )
    .await;
    let body = calls.lock().expect("calls")[0].2.clone();
    let payload: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(
        payload["body"], "0 B used, 1.0 GiB of 1.0 GiB left (daily)",
        "{payload}"
    );
}

/// RD-1291-02: a profile active from the very first sample counts its traffic, and the budget
/// running out reaches the rule in the rule's own words. Before the fix every sample started
/// again at the current total, and `budget_exhausted` never went out.
#[tokio::test]
async fn a_budget_counted_from_the_first_sample_runs_out_and_reaches_the_rule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    // The supervisor stops, so its fifteen-second bandwidth tick takes no sample of its own:
    // both samples below are the test's, whenever the test runs. The notification hub is not
    // the scheduler's and keeps delivering.
    harness.scheduler.shutdown().await.expect("shutdown");
    let router = harness.router.clone();
    let (status, profile) = common::post_json(
        &router,
        "/api/v1/bandwidth/profiles",
        json!({ "name": "Tiny", "daily_budget_bytes": "1024" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{profile}");
    let (status, schedule) = common::put_json(
        &router,
        "/api/v1/bandwidth/schedule",
        json!({ "timezone": "UTC", "default_profile_id": profile["id"], "windows": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{schedule}");
    let (_server, endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, saved) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        json!({
            "name": "budget",
            "target_id": target["id"],
            "events": ["budget_exhausted"],
            "title_template": "Budget used up ({budget_period})",
            "body_template": "{budget_used} of {budget_limit}"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    let (status, created) = common::post_json(
        &router,
        "/api/v1/downloads",
        json!({ "url": "https://example.test/traffic.bin" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id: rd_core::DownloadId = created["id"]
        .as_str()
        .expect("download id")
        .parse()
        .expect("download id");

    // The first sample, with the profile already active: it sets the odometer and charges
    // nothing from before it; the second counts the 2 KiB since.
    for committed in [512, 512 + 2048] {
        harness
            .database
            .set_download_progress(id, committed, None)
            .await
            .expect("progress");
        harness
            .scheduler
            .supervise_bandwidth()
            .await
            .expect("supervise");
    }

    let received = &calls;
    common::eventually(
        std::time::Duration::from_secs(20),
        "no budget_exhausted delivery arrived",
        || async move { (!received.lock().expect("calls").is_empty()).then_some(()) },
    )
    .await;
    let body = calls.lock().expect("calls")[0].2.clone();
    let payload: serde_json::Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(payload["event"], "budget_exhausted", "{payload}");
    assert_eq!(payload["title"], "Budget used up (daily)", "{payload}");
    assert_eq!(payload["body"], "2.0 KiB of 1.0 KiB", "{payload}");
    let deliveries = harness
        .database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries");
    assert_eq!(deliveries.len(), 1, "{deliveries:?}");
}
