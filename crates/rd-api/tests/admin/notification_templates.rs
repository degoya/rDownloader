//! A rule's own wording (RD-1280-02): the rule route keeps a title and a text template, refuses
//! one naming no placeholder with a stable code, hands out the placeholder table, and a finished
//! package reaches the webhook in the rule's words -- the JSON keeping its fields.

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
            "event", "severity", "time", "date", "title", "message", "package", "category",
            "state", "size", "path"
        ])
    );
    let started = event("download_started");
    assert_eq!(started["takes_template"], false);
    assert_eq!(started["placeholders"], json!([]));
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
