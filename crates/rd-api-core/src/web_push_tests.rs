//! The VAPID key made once and kept, a key whose vault entry is gone replaced along with the
//! subscriptions made for it, and what a round of pushes comes to (RD-1240-13).

use rd_notify::{PushOutcome, WebPushSubscription};

use super::{settle_pushes, vapid_key};

async fn stores(directory: &tempfile::TempDir) -> (rd_db::Database, rd_secrets::SecretStore) {
    let database = rd_db::Database::open(directory.path().join("push.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("vault");
    (database, secrets)
}

fn browser(endpoint: &str) -> rd_db::NewWebPushSubscription {
    rd_db::NewWebPushSubscription {
        endpoint: endpoint.to_owned(),
        p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
            .to_owned(),
        auth: "BTBZMqHH6r4Tts7J_aSIgg".to_owned(),
        device_name: "Phone".to_owned(),
        events: Vec::new(),
    }
}

#[tokio::test]
async fn the_key_is_made_once_and_kept() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, secrets) = stores(&directory).await;

    let first = vapid_key(&database, &secrets).await.expect("make");
    let again = vapid_key(&database, &secrets).await.expect("read");

    assert_eq!(again.public_key(), first.public_key());
    let stored = database.web_push_key().await.expect("row").expect("a key");
    assert_eq!(stored.public_key, first.public_key());
    assert!(secrets.get_bytes(&stored.private_key_ref).await.is_ok());
}

/// A restored backup brings the row but not the vault entry: a new key replaces it, and the
/// subscriptions made for the old key go, since a push service would refuse them.
#[tokio::test]
async fn an_unreadable_key_is_replaced_and_its_subscriptions_go() {
    let directory = tempfile::tempdir().expect("tempdir");
    let (database, secrets) = stores(&directory).await;
    let first = vapid_key(&database, &secrets).await.expect("make");
    database
        .upsert_web_push_subscription(browser("https://push.example.org/a"))
        .await
        .expect("subscribe");
    let stale = database.web_push_key().await.expect("row").expect("a key");
    secrets
        .remove(&stale.private_key_ref)
        .await
        .expect("lose it");

    let second = vapid_key(&database, &secrets).await.expect("replace");

    assert_ne!(second.public_key(), first.public_key());
    let stored = database.web_push_key().await.expect("row").expect("a key");
    assert_eq!(stored.public_key, second.public_key());
    assert_ne!(stored.private_key_ref, stale.private_key_ref);
    assert!(
        database
            .list_web_push_subscriptions()
            .await
            .expect("list")
            .is_empty()
    );
}

fn subscription(id: &str, device: &str) -> WebPushSubscription {
    WebPushSubscription {
        id: id.to_owned(),
        endpoint: format!("https://push.example.org/{id}"),
        device_name: device.to_owned(),
        events: Vec::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        p256dh: String::new(),
        auth: String::new(),
    }
}

fn failed(retryable: bool) -> PushOutcome {
    PushOutcome::Failed {
        status: Some(if retryable { 503 } else { 403 }),
        detail: "refused".to_owned(),
        retryable,
    }
}

#[test]
fn a_round_of_pushes_settles_into_one_attempt() {
    let phone = subscription("a", "Phone");
    let laptop = subscription("b", "Laptop");

    // Delivered to one, gone at the other: delivered, and the gone one is deleted.
    let (attempt, gone) = settle_pushes(vec![
        (&phone, PushOutcome::Delivered),
        (&laptop, PushOutcome::Gone),
    ]);
    assert!(attempt.ok);
    assert_eq!(gone, ["b"]);

    // Gone everywhere: nobody got it, and retrying cannot change that.
    let (attempt, gone) = settle_pushes(vec![(&phone, PushOutcome::Gone)]);
    assert!(!attempt.ok && !attempt.retryable);
    assert_eq!(gone, ["a"]);

    // One failure the push service may get over makes the delivery worth retrying.
    let (attempt, gone) = settle_pushes(vec![(&phone, failed(false)), (&laptop, failed(true))]);
    assert!(!attempt.ok && attempt.retryable);
    assert!(gone.is_empty());
    let excerpt = attempt.excerpt.expect("excerpt");
    assert!(excerpt.contains("Phone (403): refused"), "{excerpt}");
    assert!(excerpt.contains("Laptop (503): refused"), "{excerpt}");

    // A refusal alone is final.
    let (attempt, _) = settle_pushes(vec![
        (&phone, PushOutcome::Delivered),
        (&laptop, failed(false)),
    ]);
    assert!(!attempt.ok && !attempt.retryable);
}
