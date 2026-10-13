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
                title_template: None,
                body_template: None,
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
                    package_path: None,
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

/// RD-1250-01: a delivery keeps the folder of its package, and a retry after a restart -- the
/// database opened again, the worker reading what is due -- names the same folder.
#[tokio::test]
async fn a_retried_delivery_still_names_its_package_folder_after_a_restart() {
    use rd_notify::{DeliveryState, NotificationEvent, Severity, TargetKind};

    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("notifications.sqlite");
    let folder = "/srv/downloads/Holiday {2026}";
    {
        let database = Database::open(&path).await.expect("database");
        let target = database
            .upsert_notification_target(
                None,
                crate::NewNotificationTarget {
                    name: "Jellyfin".to_owned(),
                    kind: TargetKind::Plugin,
                    enabled: true,
                    endpoint: "https://jellyfin.example.test".to_owned(),
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
                    name: "Finished".to_owned(),
                    enabled: true,
                    target_id: target.id,
                    events: vec![NotificationEvent::PackageCompleted],
                    category_id: None,
                    min_severity: Severity::Info,
                    title_template: None,
                    body_template: None,
                },
            )
            .await
            .expect("rule");
        for (key, package_path) in [("with", Some(folder.to_owned())), ("without", None)] {
            database
                .queue_notification_delivery(crate::NewDelivery {
                    rule_id: rule.id,
                    target_id: rule.target_id,
                    idempotency_key: key.to_owned(),
                    event: NotificationEvent::PackageCompleted,
                    title: key.to_owned(),
                    body: "done".to_owned(),
                    package_path,
                })
                .await
                .expect("queue delivery");
        }
        let first = database
            .due_notification_deliveries(chrono::Utc::now())
            .await
            .expect("due");
        for delivery in first {
            database
                .record_notification_attempt(
                    delivery.id,
                    DeliveryState::Retrying,
                    1,
                    Some(chrono::Utc::now()),
                    Some(503),
                    None,
                )
                .await
                .expect("record attempt");
        }
        database.close().await.expect("close");
    }

    let database = Database::open(&path).await.expect("database again");
    let due = database
        .due_notification_deliveries(chrono::Utc::now() + chrono::Duration::seconds(1))
        .await
        .expect("due after the restart");
    assert_eq!(due.len(), 2, "{due:?}");
    let named = |key: &str| {
        due.iter()
            .find(|delivery| delivery.idempotency_key == key)
            .map(|delivery| (delivery.attempt, delivery.package_path.clone()))
    };
    assert_eq!(named("with"), Some((1, Some(folder.to_owned()))));
    assert_eq!(named("without"), Some((1, None)));
}

/// RD-1280-02: a rule keeps its own title and text across a reopen, and a save without them
/// returns it to the built-in wording.
#[tokio::test]
async fn a_rule_keeps_its_own_wording_until_it_is_cleared() {
    use rd_notify::{NotificationEvent, Severity, TargetKind};

    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("templates.sqlite");
    let database = Database::open(&path).await.expect("database");
    let target = database
        .upsert_notification_target(
            None,
            crate::NewNotificationTarget {
                name: "ntfy".to_owned(),
                kind: TargetKind::Apprise,
                enabled: true,
                endpoint: "ntfy".to_owned(),
                config: serde_json::json!({}),
                secret_ref: None,
                clear_secret: false,
            },
        )
        .await
        .expect("target");
    let rule = |title: Option<&str>, body: Option<&str>| crate::NewNotificationRule {
        name: "Own words".to_owned(),
        enabled: true,
        target_id: target.id,
        events: vec![NotificationEvent::PackageCompleted],
        category_id: None,
        min_severity: Severity::Info,
        title_template: title.map(str::to_owned),
        body_template: body.map(str::to_owned),
    };
    let saved = database
        .upsert_notification_rule(
            None,
            rule(Some("{package} \u{2713}"), Some("Fertig um {time}")),
        )
        .await
        .expect("rule");
    drop(database);

    let database = Database::open(&path).await.expect("reopened");
    let stored = database.list_notification_rules().await.expect("rules");
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].title_template.as_deref(),
        Some("{package} \u{2713}")
    );
    assert_eq!(stored[0].body_template.as_deref(), Some("Fertig um {time}"));

    database
        .upsert_notification_rule(Some(saved.id), rule(None, None))
        .await
        .expect("cleared");
    let stored = database.list_notification_rules().await.expect("rules");
    assert_eq!(stored[0].title_template, None);
    assert_eq!(stored[0].body_template, None);
}
