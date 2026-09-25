//! What a finished subscription poll announces on the bus (RD-106-09).
//!
//! "Check now" answers before the check has run, so the only thing that can tell an interface
//! that a particular subscription stopped being busy is the event written when the run is
//! recorded. Every other write in the subscription store emits the same anonymous
//! `{"resource":"subscription"}`, which says neither which one changed nor what happened --
//! a client could not tell a finished poll from an unrelated edit. These tests hold the
//! fields that make the difference.

use chrono::Utc;
use rd_core::EventKind;
use rd_db::{Database, NewSubscription, PollResult};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database")
}

fn subscription() -> NewSubscription {
    NewSubscription {
        name: "My Indexer".to_owned(),
        url: "https://indexer.test/api".parse().expect("url"),
        kind: rd_core::SubscriptionKind::Indexer,
        enabled: true,
        mode: rd_core::SubscriptionMode::Review,
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters::default(),
        backlog: rd_core::BacklogPolicy::default(),
        category_map: Vec::new(),
        source_categories: Vec::new(),
        every_release: false,
        view: rd_core::SubscriptionView::List,
        autoplay: false,
        card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
        schedule: None,
        secret_ref: None,
    }
}

fn result(error: Option<&str>) -> PollResult {
    PollResult {
        found: 7,
        accepted: 2,
        skipped: 5,
        error: error.map(ToOwned::to_owned),
        next_run_at: Utc::now(),
        consecutive_failures: 0,
        etag: None,
        last_modified: None,
    }
}

/// Drains the bus and returns the payload of the event that reports a finished poll.
fn finished(
    events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>,
) -> serde_json::Value {
    while let Ok(event) = events.try_recv() {
        assert_eq!(event.kind, EventKind::SubscriptionChanged);
        if event
            .payload
            .get("poll")
            .and_then(serde_json::Value::as_str)
            == Some("finished")
        {
            return event.payload;
        }
    }
    panic!("no event reported a finished poll");
}

#[tokio::test]
async fn a_finished_poll_names_its_subscription_and_what_it_found() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");

    let mut events = database.subscribe();
    database
        .finish_subscription_run(created.id, Utc::now(), result(None))
        .await
        .expect("run recorded");

    let payload = finished(&mut events);
    assert_eq!(payload["subscription_id"], created.id.to_string());
    assert_eq!(payload["found"], 7);
    assert_eq!(payload["accepted"], 2);
    assert_eq!(payload["skipped"], 5);
    assert!(payload["error"].is_null(), "{payload}");
    // Still the resource every other subscription write announces, so a client that only
    // re-reads the list keeps working.
    assert_eq!(payload["resource"], "subscription");
}

/// A failed check has to be as recognisable as a successful one, or the interface can only
/// report that something happened.
#[tokio::test]
async fn a_failed_poll_carries_its_reason() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");

    let mut events = database.subscribe();
    database
        .finish_subscription_run(created.id, Utc::now(), result(Some("poll timed out")))
        .await
        .expect("run recorded");

    let payload = finished(&mut events);
    assert_eq!(payload["error"], "poll timed out");
}

/// An ordinary edit must not look like a finished check, or every change would clear the
/// busy state of a subscription that is still being polled.
#[tokio::test]
async fn an_ordinary_change_does_not_claim_a_poll_finished() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");

    let mut events = database.subscribe();
    database
        .set_subscription_enabled(created.id, false)
        .await
        .expect("disabled");

    let event = events.try_recv().expect("an event");
    assert_eq!(event.kind, EventKind::SubscriptionChanged);
    assert!(event.payload.get("poll").is_none(), "{}", event.payload);
}
