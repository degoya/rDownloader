//! Clearing logs, audit records and statistics over REST (RD-120-34), and the notification
//! history (RD-130-08).
//!
//! What is checked here is the contract a client sees: the count arrives before the question,
//! an unconfirmed request is refused with a stable code, each action empties its own store and
//! nothing else, the audit clear writes itself into the emptied log, a notification still owed
//! an attempt survives its history being cleared, and the queue is not touched by any of them.
//!
//! The three-way isolation measured as store counts — including the transfer statistics, which
//! have no public write — is in `crates/rd-db/tests/data_reset.rs`.

mod common;

use std::collections::BTreeMap;

use axum::http::StatusCode;
use chrono::Utc;
use common::{get_json, post_json, test_harness};
use rd_core::{AuditAction, AuditOutcome, LogLevel};
use rd_db::{NewAuditRecord, NewDelivery, NewLogRecord};
use rd_notify::{DeliveryState, NotificationEvent};
use serde_json::json;

const CONFIRMED: fn() -> serde_json::Value = || json!({ "confirmed": true });

/// Every clear this file knows, so a check that holds for all of them names all of them.
const CLEARS: [&str; 4] = [
    "/api/v1/diagnostics/logs/clear",
    "/api/v1/audit/records/clear",
    "/api/v1/stats/transfers/clear",
    "/api/v1/notifications/deliveries/clear",
];

async fn seed(database: &rd_db::Database) {
    database
        .append_log_records(
            (0..4)
                .map(|index| NewLogRecord {
                    recorded_at: Utc::now(),
                    level: LogLevel::Info,
                    component: "rd_api".to_owned(),
                    code: None,
                    correlation_id: None,
                    message: format!("seeded line {index}"),
                    fields: BTreeMap::new(),
                })
                .collect(),
        )
        .await
        .expect("logs");
    for action in [AuditAction::LoginSucceeded, AuditAction::TokenCreated] {
        database
            .append_audit_record(NewAuditRecord::new(action, AuditOutcome::Success))
            .await
            .expect("audit");
    }
}

async fn preview(router: &axum::Router) -> serde_json::Value {
    let (status, body) = get_json(router, "/api/v1/system/data-reset").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

#[tokio::test]
async fn the_preview_names_the_numbers_before_anything_is_cleared() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed(&harness.database).await;

    let counts = preview(&harness.router).await;

    assert_eq!(counts["logs"], 4, "{counts}");
    // The two seeded records plus whatever the harness's own startup wrote; what matters is
    // that the figure is there and is not the log count.
    assert!(
        counts["audit"].as_u64().expect("audit count") >= 2,
        "{counts}"
    );
    assert!(counts["stats"].is_number(), "{counts}");
    assert_eq!(counts["notifications"], 0, "{counts}");
}

#[tokio::test]
async fn an_unconfirmed_clear_is_refused_and_removes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed(&harness.database).await;

    for path in CLEARS {
        let (status, body) = post_json(&harness.router, path, json!({ "confirmed": false })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
        assert_eq!(body["code"], "data_reset.not_confirmed", "{path}: {body}");
        let (status, body) = post_json(&harness.router, path, json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
        assert_eq!(body["code"], "data_reset.not_confirmed", "{path}: {body}");
    }

    assert_eq!(
        harness.database.count_log_records().await.expect("logs"),
        4,
        "a refused clear removed log records anyway"
    );
}

#[tokio::test]
async fn clearing_the_logs_reports_the_count_and_leaves_the_audit_standing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed(&harness.database).await;
    let audit_before = harness.database.count_audit_records().await.expect("audit");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/diagnostics/logs/clear",
        CONFIRMED(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 4, "{body}");
    assert_eq!(preview(&harness.router).await["logs"], 0);
    // One more than before: the clear audited itself like every other destructive action.
    assert_eq!(
        harness.database.count_audit_records().await.expect("audit"),
        audit_before + 1
    );
}

#[tokio::test]
async fn clearing_the_audit_writes_itself_into_the_emptied_log() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed(&harness.database).await;
    let before = harness.database.count_audit_records().await.expect("count");
    assert!(before >= 2);

    let (status, body) =
        post_json(&harness.router, "/api/v1/audit/records/clear", CONFIRMED()).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], before, "{body}");

    let (status, records) = get_json(&harness.router, "/api/v1/audit/records?limit=50").await;
    assert_eq!(status, StatusCode::OK, "{records}");
    let rows = records["records"].as_array().expect("records");
    assert_eq!(
        rows.len(),
        1,
        "the emptied log holds exactly the entry that explains it: {records}"
    );
    let entry = &rows[0];
    assert_eq!(entry["action"], "audit_cleared", "{entry}");
    assert_eq!(entry["outcome"], "success", "{entry}");
    assert_eq!(
        entry["details"][rd_db::CLEARED_DETAIL_KEY],
        before.to_string(),
        "the entry says how many records went: {entry}"
    );
    assert!(
        entry["recorded_at"]
            .as_str()
            .is_some_and(|at| !at.is_empty()),
        "the entry carries when: {entry}"
    );
    // The service log is a different store and is not what was asked for.
    assert_eq!(preview(&harness.router).await["logs"], 4);
}

#[tokio::test]
async fn clearing_the_statistics_touches_neither_log() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed(&harness.database).await;
    let audit_before = harness.database.count_audit_records().await.expect("audit");

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/stats/transfers/clear",
        CONFIRMED(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["removed"].is_number(), "{body}");
    assert_eq!(preview(&harness.router).await["logs"], 4);
    assert_eq!(
        harness.database.count_audit_records().await.expect("audit"),
        audit_before + 1,
        "only the record of the clear itself was added"
    );
}

/// Seeds a delivered, a failed and a retrying delivery straight into the store.
///
/// None is left `queued`: the harness runs the delivery worker, which would pick a due row up
/// within its sweep and move it on. The retrying one is due in an hour, so the worker leaves it
/// alone for the length of the test. The queued case is in `crates/rd-db/tests/data_reset.rs`,
/// where no worker runs.
async fn seed_deliveries(database: &rd_db::Database) {
    let rule_id = rd_core::NotificationRuleId::new();
    let target_id = rd_core::NotificationTargetId::new();
    for state in [
        DeliveryState::Delivered,
        DeliveryState::Failed,
        DeliveryState::Retrying,
    ] {
        let title = format!("{state:?}");
        database
            .queue_notification_delivery(NewDelivery {
                rule_id,
                target_id,
                idempotency_key: format!("seed-{title}"),
                event: NotificationEvent::BudgetExhausted,
                title: title.clone(),
                body: String::new(),
            })
            .await
            .expect("queue");
        let delivery = database
            .list_notification_deliveries(50)
            .await
            .expect("list")
            .into_iter()
            .find(|delivery| delivery.title == title)
            .expect("the delivery just queued");
        database
            .record_notification_attempt(
                delivery.id,
                state,
                1,
                (state == DeliveryState::Retrying).then(|| Utc::now() + chrono::Duration::hours(1)),
                None,
                None,
            )
            .await
            .expect("attempt");
    }
}

#[tokio::test]
async fn clearing_the_notification_history_keeps_a_retry_and_audits_itself() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed(&harness.database).await;
    seed_deliveries(&harness.database).await;
    assert_eq!(
        preview(&harness.router).await["notifications"],
        2,
        "the count names what goes, not the whole table"
    );

    let (status, body) = post_json(
        &harness.router,
        "/api/v1/notifications/deliveries/clear",
        CONFIRMED(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 2, "{body}");
    let (status, history) =
        get_json(&harness.router, "/api/v1/notifications/deliveries?limit=50").await;
    assert_eq!(status, StatusCode::OK, "{history}");
    let states: Vec<&str> = history
        .as_array()
        .expect("a list of deliveries")
        .iter()
        .filter_map(|delivery| delivery["state"].as_str())
        .collect();
    assert_eq!(
        states,
        ["retrying"],
        "a notification not yet sent is not history: {history}"
    );
    assert_eq!(preview(&harness.router).await["notifications"], 0);
    assert_eq!(preview(&harness.router).await["logs"], 4);

    let (status, records) = get_json(
        &harness.router,
        "/api/v1/audit/records?action=notifications_cleared&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{records}");
    let rows = records["records"].as_array().expect("records");
    assert_eq!(rows.len(), 1, "the clear is in the audit log: {records}");
    assert_eq!(rows[0]["outcome"], "success", "{records}");
    assert_eq!(
        rows[0]["details"][rd_db::CLEARED_DETAIL_KEY],
        "2",
        "the entry says how many went: {records}"
    );
}

/// The worry a delete button always raises, answered rather than asserted.
#[tokio::test]
async fn every_clear_leaves_the_queue_and_the_settings_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    seed(&harness.database).await;
    let (status, added) = post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": "https://example.invalid/kept.bin" }),
    )
    .await;
    assert!(status.is_success(), "{added}");
    let download_id = added["id"].as_str().expect("id").to_owned();
    let (status, settings_before) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings_before}");

    for path in CLEARS {
        let (status, body) = post_json(&harness.router, path, CONFIRMED()).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    let (status, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    assert_eq!(status, StatusCode::OK, "{downloads}");
    let ids: Vec<&str> = downloads
        .as_array()
        .expect("a list of downloads")
        .iter()
        .filter_map(|download| download["id"].as_str())
        .collect();
    assert!(
        ids.contains(&download_id.as_str()),
        "the download went with the logs: {downloads}"
    );
    let (status, settings_after) = get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings_after}");
    assert_eq!(settings_before, settings_after, "the settings changed");
}
