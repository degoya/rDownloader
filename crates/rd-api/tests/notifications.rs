//! Integration tests for the notification hub (RD-050-14): a target's secret never comes
//! back out, a signed webhook actually arrives, one event produces at most one delivery per
//! rule, and a failing target neither blocks the others nor loses its history.

mod common;

use std::sync::{Arc, Mutex};

use axum::{Router, http::StatusCode, routing::post};

const SECRET: &str = "topsecret-signing-key";

/// One received webhook call: signature, idempotency key and body.
type Call = (String, String, String);

/// Starts a local receiver and returns its URL together with what it collects.
async fn spawn_receiver() -> (String, Arc<Mutex<Vec<Call>>>) {
    let calls: Arc<Mutex<Vec<Call>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&calls);
    let app = Router::new().route(
        "/hook",
        post(move |headers: axum::http::HeaderMap, body: String| {
            let sink = Arc::clone(&sink);
            async move {
                let header = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_owned()
                };
                sink.lock().expect("sink").push((
                    header(rd_notify::SIGNATURE_HEADER),
                    header(rd_notify::IDEMPOTENCY_HEADER),
                    body,
                ));
                StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}/hook"), calls)
}

async fn create_target(router: &Router, body: serde_json::Value) -> serde_json::Value {
    let (status, target) = common::post_json(router, "/api/v1/notifications/targets", body).await;
    assert_eq!(status, StatusCode::CREATED, "{target}");
    target
}

#[tokio::test]
async fn a_target_secret_is_stored_but_never_returned() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let target = create_target(
        &router,
        serde_json::json!({
            "name": "Ops webhook",
            "kind": "webhook",
            "endpoint": "https://hooks.example.com/rd",
            "secret": SECRET
        }),
    )
    .await;
    assert_eq!(target["has_secret"], true);
    assert!(!target.to_string().contains(SECRET), "{target}");

    let (status, listed) = common::get_json(&router, "/api/v1/notifications/targets").await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(!listed.to_string().contains(SECRET), "{listed}");
    assert!(!listed.to_string().contains("secret_ref"), "{listed}");
}

#[tokio::test]
async fn a_webhook_url_has_to_be_http_or_https() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, refused) = common::post_json(
        &router,
        "/api/v1/notifications/targets",
        serde_json::json!({ "name": "bad", "kind": "webhook", "endpoint": "file:///etc/passwd" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "notification.endpoint_invalid");
}

#[tokio::test]
async fn a_rule_needs_a_target_that_exists() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (status, refused) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        serde_json::json!({
            "name": "orphan",
            "target_id": rd_core::NotificationTargetId::new(),
            "events": ["package_failed"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "notification.target_not_found");
}

#[tokio::test]
async fn deleting_a_target_takes_its_rules_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let target = create_target(
        &router,
        serde_json::json!({
            "name": "Ops webhook",
            "kind": "webhook",
            "endpoint": "https://hooks.example.com/rd"
        }),
    )
    .await;
    let id = target["id"].as_str().expect("id");
    let (status, rule) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        serde_json::json!({ "name": "everything", "target_id": id }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");

    let (status, deleted) =
        common::delete_json(&router, &format!("/api/v1/notifications/targets/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    let (_, rules) = common::get_json(&router, "/api/v1/notifications/rules").await;
    assert!(
        rules.as_array().expect("rules").is_empty(),
        "a rule without a target could never deliver: {rules}"
    );
}

#[tokio::test]
async fn a_test_delivery_reaches_a_local_webhook_and_carries_its_signature() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        serde_json::json!({
            "name": "local",
            "kind": "webhook",
            "endpoint": endpoint,
            "secret": SECRET
        }),
    )
    .await;
    let id = target["id"].as_str().expect("id");

    let (status, result) = common::post_json(
        &router,
        &format!("/api/v1/notifications/targets/{id}/test"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["status"], 200);

    let calls = calls.lock().expect("calls");
    let (signature, idempotency, body) = calls.first().expect("the receiver was called");
    // The receiver has to be able to verify the call came from here, and to drop a repeat.
    assert!(signature.starts_with("sha256="), "{signature}");
    assert!(!idempotency.is_empty());
    // The signing key itself never travels — only the HMAC does.
    assert!(!signature.contains(SECRET));
    assert!(!body.contains(SECRET), "{body}");
}

#[tokio::test]
async fn a_failing_target_records_its_attempt_without_leaking_the_secret() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    // Port 1 is reserved and refuses immediately, so the attempt fails without a wait.
    let target = create_target(
        &router,
        serde_json::json!({
            "name": "dead",
            "kind": "webhook",
            "endpoint": "http://127.0.0.1:1/hook",
            "secret": SECRET
        }),
    )
    .await;
    let id = target["id"].as_str().expect("id");
    let (status, result) = common::post_json(
        &router,
        &format!("/api/v1/notifications/targets/{id}/test"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["ok"], false, "{result}");
    assert!(result["detail"].as_str().is_some_and(|d| !d.is_empty()));
    assert!(!result.to_string().contains(SECRET), "{result}");
}

/// `budget_exhausted` reaches a rule that asks for it (RD-120-62): the scheduler announces a
/// budget running out as `bandwidth.changed` with `entity: "budget"`, and only that edge
/// becomes a delivery — a profile edit or the budget coming back notifies nobody.
#[tokio::test]
async fn a_used_up_traffic_budget_reaches_a_rule_that_asks_for_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = harness.router.clone();
    let (endpoint, calls) = spawn_receiver().await;
    let target = create_target(
        &router,
        serde_json::json!({ "name": "local", "kind": "webhook", "endpoint": endpoint }),
    )
    .await;
    let (status, rule) = common::post_json(
        &router,
        "/api/v1/notifications/rules",
        serde_json::json!({
            "name": "budget",
            "target_id": target["id"],
            "events": ["budget_exhausted"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rule}");

    let profile = rd_core::BandwidthProfileId::new();
    for payload in [
        serde_json::json!({ "entity": "profile", "id": profile }),
        serde_json::json!({ "entity": "budget", "exhausted": false, "profile": profile }),
        serde_json::json!({
            "entity": "budget",
            "exhausted": true,
            "profile": profile,
            "period": "daily",
            "used_bytes": 600,
            "limit_bytes": 500
        }),
    ] {
        harness.database.broadcast(rd_core::EventEnvelope::new(
            rd_core::EventKind::BandwidthChanged,
            payload,
        ));
    }

    // The hub sweeps its queue every five seconds; the call lands within one sweep.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    let body = loop {
        if let Some((_, _, body)) = calls.lock().expect("calls").first().cloned() {
            break body;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no budget_exhausted delivery arrived"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let body: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(body["event"], "budget_exhausted", "{body}");
    assert!(
        body["body"]
            .as_str()
            .is_some_and(|text| text.contains("daily")),
        "{body}"
    );
    // The events are handled in order, so the two before it have been seen already.
    let deliveries = harness
        .database
        .list_notification_deliveries(100)
        .await
        .expect("deliveries");
    assert_eq!(deliveries.len(), 1, "{deliveries:?}");
    assert_eq!(
        deliveries[0].event,
        rd_notify::NotificationEvent::BudgetExhausted
    );
}
