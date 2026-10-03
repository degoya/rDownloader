//! A notice reaches a rule once, even after its delivery left the history (RD-190-19).

use rd_notify::{NotificationEvent, Severity, TargetKind};

use crate::{Database, NewDelivery};

#[tokio::test]
async fn a_notice_is_queued_once_per_rule_even_after_the_history_was_cleared() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("notices.sqlite"))
        .await
        .expect("database");
    let target = database
        .upsert_notification_target(
            None,
            crate::NewNotificationTarget {
                name: "Webhook".to_owned(),
                kind: TargetKind::Webhook,
                enabled: true,
                endpoint: "https://example.test/hook".to_owned(),
                config: serde_json::json!({}),
                secret_ref: None,
                clear_secret: false,
            },
        )
        .await
        .expect("target");
    let rule = database
        .upsert_notification_rule(
            None,
            crate::NewNotificationRule {
                name: "Updates".to_owned(),
                enabled: true,
                target_id: target.id,
                events: vec![NotificationEvent::UpdateAvailable],
                category_id: None,
                min_severity: Severity::Info,
            },
        )
        .await
        .expect("rule");
    let notice = |version: &str| {
        vec![NewDelivery {
            rule_id: rule.id,
            target_id: rule.target_id,
            idempotency_key: rd_notify::idempotency_key(
                rule.id,
                &format!("update_available:{version}"),
            ),
            event: NotificationEvent::UpdateAvailable,
            title: format!("rDownloader {version} is available"),
            body: String::new(),
        }]
    };

    assert_eq!(
        database
            .queue_notification_notice(notice("9.9.9"))
            .await
            .expect("first"),
        1
    );
    assert_eq!(
        database
            .queue_notification_notice(notice("9.9.9"))
            .await
            .expect("second"),
        0,
        "the next check finds the same version and queues nothing"
    );

    // The delivery leaves the history; the notice is still remembered.
    let delivered = database
        .list_notification_deliveries(10)
        .await
        .expect("deliveries");
    assert_eq!(delivered.len(), 1);
    database
        .record_notification_attempt(
            delivered[0].id,
            rd_notify::DeliveryState::Delivered,
            1,
            None,
            Some(200),
            None,
        )
        .await
        .expect("attempt");
    assert_eq!(
        database
            .clear_notification_deliveries()
            .await
            .expect("clear"),
        1
    );
    assert_eq!(
        database
            .queue_notification_notice(notice("9.9.9"))
            .await
            .expect("after the clear"),
        0,
        "a cleared history must not announce the same version again"
    );
    assert_eq!(
        database
            .queue_notification_notice(notice("9.9.10"))
            .await
            .expect("newer"),
        1,
        "a newer version is a new notice"
    );
}
