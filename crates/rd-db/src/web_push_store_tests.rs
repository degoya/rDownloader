//! The push subscriptions and the VAPID key (RD-1240-13): a browser subscribing again updates its
//! row, the first subscription makes the `web_push` target and its rule once, and a replaced key
//! takes the subscriptions made for the old one with it.

use rd_notify::{NotificationEvent, TargetKind};

use crate::{Database, NewWebPushSubscription, WebPushKey};

fn browser(endpoint: &str, device: &str) -> NewWebPushSubscription {
    NewWebPushSubscription {
        endpoint: endpoint.to_owned(),
        p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
            .to_owned(),
        auth: "BTBZMqHH6r4Tts7J_aSIgg".to_owned(),
        device_name: device.to_owned(),
        events: Vec::new(),
    }
}

async fn database(directory: &tempfile::TempDir) -> Database {
    Database::open(directory.path().join("push.sqlite3"))
        .await
        .expect("database")
}

#[tokio::test]
async fn subscribing_again_updates_the_row_and_survives_a_reopen() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("push.sqlite3");
    let database = Database::open(&path).await.expect("database");
    assert!(
        database
            .list_web_push_subscriptions()
            .await
            .expect("list")
            .is_empty()
    );

    let first = database
        .upsert_web_push_subscription(browser("https://push.example.org/a", "Phone"))
        .await
        .expect("subscribe");
    let mut again = browser("https://push.example.org/a", "Phone (Firefox)");
    again.events = vec![NotificationEvent::PackageFailed];
    let second = database
        .upsert_web_push_subscription(again)
        .await
        .expect("subscribe again");
    database
        .upsert_web_push_subscription(browser("https://push.example.org/b", "Desktop"))
        .await
        .expect("second browser");
    database.close().await.expect("close");

    let database = Database::open(&path).await.expect("reopen");
    let stored = database.list_web_push_subscriptions().await.expect("list");
    assert_eq!(stored.len(), 2);
    assert_eq!(second.id, first.id, "the same push address is the same row");
    assert_eq!(stored[0].id, first.id);
    assert_eq!(stored[0].device_name, "Phone (Firefox)");
    assert_eq!(stored[0].events, [NotificationEvent::PackageFailed]);
    assert_eq!(stored[0].auth, "BTBZMqHH6r4Tts7J_aSIgg");
    assert_eq!(stored[1].device_name, "Desktop");
}

#[tokio::test]
async fn the_first_subscription_makes_the_target_and_its_rule_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    // A target of another kind already holds the name.
    database
        .upsert_notification_target(
            None,
            crate::NewNotificationTarget {
                name: "Browser push".to_owned(),
                kind: TargetKind::Webhook,
                enabled: true,
                endpoint: "https://hooks.example.org/x".to_owned(),
                config: serde_json::json!({}),
                secret_ref: None,
                clear_secret: false,
            },
        )
        .await
        .expect("webhook");

    for endpoint in ["https://push.example.org/a", "https://push.example.org/b"] {
        database
            .upsert_web_push_subscription(browser(endpoint, "Browser"))
            .await
            .expect("subscribe");
    }

    let targets = database.list_notification_targets().await.expect("targets");
    let push: Vec<_> = targets
        .iter()
        .filter(|target| target.kind == TargetKind::WebPush)
        .collect();
    assert_eq!(push.len(), 1, "{targets:?}");
    assert_eq!(push[0].name, "Browser push 2");
    assert!(push[0].enabled && !push[0].has_secret);
    let rules = database.list_notification_rules().await.expect("rules");
    let rules: Vec<_> = rules
        .iter()
        .filter(|rule| rule.target_id == push[0].id)
        .collect();
    assert_eq!(rules.len(), 1);
    assert!(rules[0].enabled && rules[0].events.is_empty());
    assert_eq!(rules[0].min_severity, rd_notify::Severity::Info);
}

#[tokio::test]
async fn a_deleted_subscription_is_gone_and_a_second_delete_is_not_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let subscription = database
        .upsert_web_push_subscription(browser("https://push.example.org/a", "Phone"))
        .await
        .expect("subscribe");

    database
        .delete_web_push_subscription(&subscription.id)
        .await
        .expect("delete");
    assert!(
        database
            .list_web_push_subscriptions()
            .await
            .expect("list")
            .is_empty()
    );
    let again = database
        .delete_web_push_subscription(&subscription.id)
        .await
        .expect_err("already gone");
    assert_eq!(
        crate::store_kind(&again),
        Some(crate::StoreErrorKind::NotFound),
        "{again:#}"
    );
}

fn key(reference: &str) -> WebPushKey {
    WebPushKey {
        private_key_ref: reference.to_owned(),
        public_key: format!("public-{reference}"),
    }
}

#[tokio::test]
async fn the_first_key_wins_and_a_replaced_key_takes_the_subscriptions_along() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    assert_eq!(database.web_push_key().await.expect("read"), None);

    let first = database
        .store_web_push_key(key("first"), None)
        .await
        .expect("store");
    assert_eq!(first, key("first"));
    // A second first request keeps the key already stored.
    let kept = database
        .store_web_push_key(key("second"), None)
        .await
        .expect("store again");
    assert_eq!(kept, key("first"));

    database
        .upsert_web_push_subscription(browser("https://push.example.org/a", "Phone"))
        .await
        .expect("subscribe");
    // Replacing a key that is no longer the one in force changes nothing.
    let unchanged = database
        .store_web_push_key(key("third"), Some("elsewhere".to_owned()))
        .await
        .expect("stale replace");
    assert_eq!(unchanged, key("first"));
    assert_eq!(
        database
            .list_web_push_subscriptions()
            .await
            .expect("list")
            .len(),
        1
    );

    let replaced = database
        .store_web_push_key(key("fourth"), Some("first".to_owned()))
        .await
        .expect("replace");
    assert_eq!(replaced, key("fourth"));
    assert_eq!(
        database.web_push_key().await.expect("read"),
        Some(key("fourth"))
    );
    assert!(
        database
            .list_web_push_subscriptions()
            .await
            .expect("list")
            .is_empty()
    );
}
