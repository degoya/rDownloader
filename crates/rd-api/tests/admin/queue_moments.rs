//! The whole queue's moments (RD-1290-04): a pause by hand reaches a rule that names it once,
//! however often it is set again, and its end once; the automations' three queue triggers fire
//! on their `queue.state` event, a pause's cause is the condition field `source`, and a queue
//! trigger refuses a package action.

use crate::common;
use crate::notifications::{create_target, spawn_receiver};

use std::time::Duration;

use axum::http::StatusCode;
use rd_core::{EventEnvelope, EventKind};
use serde_json::json;

/// The events of the webhook calls received so far, in order.
fn events(calls: &std::sync::Mutex<Vec<(String, String, String)>>) -> Vec<(String, String)> {
    calls
        .lock()
        .expect("calls")
        .iter()
        .map(|(_, _, body)| {
            let payload: serde_json::Value = serde_json::from_str(body).expect("JSON");
            (
                payload["event"].as_str().unwrap_or_default().to_owned(),
                payload["body"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_pause_by_hand_is_one_notification_and_its_end_another() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let router = harness.router.clone();
    let (_server, endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, rule) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        json!({
            "name": "queue",
            "target_id": target["id"],
            "events": ["queue_paused", "queue_resumed", "queue_finished"],
            "body_template": "{message} ({reason})"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");
    // The parked queue never starts it: the file waits, the queue is busy.
    let (status, created) = common::post_json(
        &router,
        "/api/v1/downloads",
        json!({ "url": "https://example.test/queued.bin", "package_name": "Queued" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    // The power supervisor's first read only learns the state, a start being no moment: the
    // pause comes after one, which read the queue unpaused.
    let supervisor = &harness.state.power_supervisor;
    common::eventually(
        Duration::from_secs(20),
        "the power supervisor never read the queue",
        || async move { (supervisor.queue_state_reads() > 0).then_some(()) },
    )
    .await;

    let (status, paused) =
        common::put_json(&router, "/api/v1/queue/pause", json!({ "minutes": 30 })).await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    let received = &calls;
    common::eventually(
        Duration::from_secs(20),
        "queue_paused did not arrive",
        || async move { (!events(received).is_empty()).then_some(()) },
    )
    .await;
    // Set again with another end: the same pause, no second moment.
    let (status, again) =
        common::put_json(&router, "/api/v1/queue/pause", json!({ "minutes": 60 })).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let (status, resumed) = common::delete_json(&router, "/api/v1/queue/pause").await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    common::eventually(
        Duration::from_secs(20),
        "queue_resumed did not arrive",
        || async move { (events(received).len() >= 2).then_some(()) },
    )
    .await;

    assert_eq!(
        events(&calls),
        [
            (
                "queue_paused".to_owned(),
                "The queue was paused by hand. (manual)".to_owned()
            ),
            (
                "queue_resumed".to_owned(),
                "The queue runs again. ()".to_owned()
            ),
        ],
        "one pause, one end, and the file still waits: not finished"
    );
}

/// Creates an enabled automation on `trigger` that starts the queue, and returns its id.
async fn automation(
    router: &axum::Router,
    trigger: &str,
    condition: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    common::post_json(
        router,
        "/api/v1/automations",
        json!({
            "name": format!("On {trigger}"),
            "enabled": true,
            "trigger": trigger,
            "condition": condition,
            "actions": [{ "kind": "start_queue" }]
        }),
    )
    .await
}

async fn runs(router: &axum::Router, id: &str) -> usize {
    let (_, runs) = common::get_json(
        router,
        &format!("/api/v1/automations/runs?automation_id={id}"),
    )
    .await;
    runs.as_array().map(Vec::len).unwrap_or_default()
}

#[tokio::test]
async fn the_queue_triggers_fire_on_their_moment() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = harness.router.clone();
    let always = json!({ "type": "always" });
    let scheduled = json!({
        "type": "predicate",
        "predicate": { "field": "source", "operator": "equals", "value": "schedule" }
    });
    let mut ids = Vec::new();
    for (trigger, condition) in [
        ("queue_paused", &scheduled),
        ("queue_resumed", &always),
        ("queue_finished", &always),
    ] {
        let (status, created) = automation(&router, trigger, condition.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        ids.push((trigger, created["id"].as_str().expect("id").to_owned()));
    }
    for payload in [
        json!({ "state": "paused", "reason": "schedule" }),
        json!({ "state": "resumed" }),
        json!({ "state": "finished" }),
    ] {
        harness
            .database
            .broadcast(EventEnvelope::new(EventKind::QueueState, payload));
    }
    for (trigger, id) in &ids {
        let router = &router;
        common::eventually(
            common::WAIT,
            &format!("{trigger} did not fire"),
            || async move { (runs(router, id).await >= 1).then_some(()) },
        )
        .await;
    }
}

#[tokio::test]
async fn a_queue_trigger_refuses_a_package_action() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, refused) = common::post_json(
        &router,
        "/api/v1/automations",
        json!({
            "name": "Nothing to act on",
            "enabled": true,
            "trigger": "queue_finished",
            "condition": { "type": "always" },
            "actions": [{ "kind": "extract_package" }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(
        refused["code"], "automation.action_needs_package",
        "{refused}"
    );
}
