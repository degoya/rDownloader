//! Notification deliveries.

use crate::Database;

/// The delivery history is trimmed in the write that queues a delivery, and a delivery the
/// worker still owes an attempt is never the row that gets dropped.
#[tokio::test]
async fn notification_deliveries_are_trimmed_per_rule_but_never_while_pending() {
    use rd_notify::{DeliveryState, NotificationEvent, Severity, TargetKind};

    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("notifications.sqlite"))
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
                name: "Everything".to_owned(),
                enabled: true,
                target_id: target.id,
                events: vec![NotificationEvent::PackageCompleted],
                category_id: None,
                min_severity: Severity::Info,
            },
        )
        .await
        .expect("rule");

    async fn queue(database: &Database, rule: &rd_notify::NotificationRule, index: i64) {
        assert!(
            database
                .queue_notification_delivery(crate::NewDelivery {
                    rule_id: rule.id,
                    target_id: rule.target_id,
                    idempotency_key: format!("event-{index}"),
                    event: NotificationEvent::PackageCompleted,
                    title: format!("Package {index}"),
                    body: "done".to_owned(),
                })
                .await
                .expect("queue delivery"),
            "each key is fresh, so each insert must be a new row"
        );
    }
    async fn deliveries(database: &Database) -> Vec<rd_notify::Delivery> {
        database
            .list_notification_deliveries(u32::MAX)
            .await
            .expect("deliveries")
    }

    let cap = crate::notify_store::MAX_DELIVERIES_PER_RULE;
    for index in 0..=cap {
        queue(&database, &rule, index).await;
    }
    assert_eq!(
        deliveries(&database).await.len() as i64,
        cap + 1,
        "nothing was attempted yet, so the trim had nothing it was allowed to drop"
    );

    // Settle everything but the very first one; that one stays queued and must survive.
    for delivery in deliveries(&database).await {
        if delivery.title != "Package 0" {
            database
                .record_notification_attempt(
                    delivery.id,
                    DeliveryState::Delivered,
                    1,
                    None,
                    Some(200),
                    None,
                )
                .await
                .expect("record attempt");
        }
    }
    queue(&database, &rule, cap + 1).await;

    let stored = deliveries(&database).await;
    assert_eq!(
        stored.len() as i64,
        cap + 1,
        "the trim keeps the newest {cap} plus the pending row it refused to drop"
    );
    assert!(
        stored.iter().any(
            |delivery| delivery.title == "Package 0" && delivery.state == DeliveryState::Queued
        ),
        "trimming a queued delivery would discard the notification, not just its record"
    );
}
