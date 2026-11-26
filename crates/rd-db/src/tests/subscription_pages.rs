//! A subscription's item pages read up to a watermark (RD-1250-13) and along an index, and how
//! many of its polls are kept (RD-1480-19).

use rd_core::{SignedDuration, SubscriptionItemState, Timestamp};

use super::subscriptions::{new_item, new_subscription};
use crate::Database;

#[tokio::test]
async fn a_page_until_a_watermark_leaves_out_items_found_after_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-until.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");
    database
        .record_subscription_items(subscription.id, vec![new_item("a"), new_item("b")])
        .await
        .expect("first items");
    let pending = Some(SubscriptionItemState::Pending);
    let seen = database
        .subscription_item_page(subscription.id, pending, 50, 0)
        .await
        .expect("first read");
    let watermark = seen.newest_discovered_at.expect("watermark");
    database
        .record_subscription_items(subscription.id, vec![new_item("later")])
        .await
        .expect("later item");

    let held = database
        .subscription_item_page_until(subscription.id, pending, 50, 0, Some(watermark))
        .await
        .expect("held read");
    let mut keys: Vec<_> = held
        .items
        .iter()
        .map(|item| item.item_key.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["a", "b"]);
    assert_eq!(held.total, 2);
    assert_eq!(held.newest_discovered_at, Some(watermark));

    let live = database
        .subscription_item_page_until(subscription.id, pending, 50, 0, None)
        .await
        .expect("live read");
    assert_eq!(live.total, 3);
    assert_eq!(live.items[0].item_key, "later");
}

/// The page of one state walks `subscription_items_state_idx`; with the optional conditions
/// written as `? IS NULL OR ...` it sorted every item of the subscription. The statements are the
/// store's own shapes, so a plan that falls back to the whole subscription fails here.
#[tokio::test]
async fn the_page_of_one_state_is_read_along_its_index() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    for until in [None, Some(Timestamp::now())] {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
            "EXPLAIN QUERY PLAN SELECT id, title FROM subscription_items \
             WHERE subscription_id = ",
        );
        query
            .push_bind("subscription")
            .push(" AND state = ")
            .push_bind("pending");
        if let Some(until) = until {
            query.push(" AND discovered_at <= ").push_bind(until);
        }
        query.push(" ORDER BY discovered_at DESC, id DESC LIMIT 50 OFFSET 0");
        let plan: Vec<(i64, i64, i64, String)> = query
            .build_query_as()
            .fetch_all(&database.readers)
            .await
            .expect("plan");
        assert!(
            plan[0]
                .3
                .contains("subscription_items_state_idx (subscription_id=? AND state=?"),
            "{plan:?}"
        );
    }
}

/// Filter, total, watermark and order answer the same for every combination of conditions.
#[tokio::test]
async fn a_page_holds_exactly_the_rows_its_conditions_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");
    let other = database
        .create_subscription(new_subscription("Other"))
        .await
        .expect("subscription");
    database
        .record_subscription_items(other.id, vec![new_item("elsewhere")])
        .await
        .expect("items");
    let skipped = crate::NewSubscriptionItem {
        state: SubscriptionItemState::Skipped,
        ..new_item("skipped")
    };
    database
        .record_subscription_items(subscription.id, vec![new_item("a"), skipped, new_item("b")])
        .await
        .expect("items");
    let past = Timestamp::now() - SignedDuration::from_hours(1);
    for (state, until, expected) in [
        (None, None, 3),
        (Some(SubscriptionItemState::Pending), None, 2),
        (Some(SubscriptionItemState::Skipped), None, 1),
        (Some(SubscriptionItemState::Queued), None, 0),
        (None, Some(past), 0),
        (Some(SubscriptionItemState::Pending), Some(past), 0),
        (
            Some(SubscriptionItemState::Pending),
            Some(Timestamp::now()),
            2,
        ),
    ] {
        let page = database
            .subscription_item_page_until(subscription.id, state, 50, 0, until)
            .await
            .expect("page");
        assert_eq!(page.total, expected, "{state:?} until {until:?}");
        assert_eq!(
            page.items.len() as u64,
            expected,
            "{state:?} until {until:?}"
        );
        assert_eq!(page.newest_discovered_at.is_some(), expected > 0);
        assert!(page.items.is_sorted_by(|a, b| {
            (a.discovered_at, a.id.to_string()) >= (b.discovered_at, b.id.to_string())
        }));
    }
    let second = database
        .subscription_item_page(subscription.id, None, 2, 2)
        .await
        .expect("second page");
    assert_eq!((second.total, second.items.len()), (3, 1));
}

#[tokio::test]
async fn the_newest_polls_are_kept_and_the_rest_dropped() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    let subscription = database
        .create_subscription(new_subscription("Busy"))
        .await
        .expect("subscription");
    let quiet = database
        .create_subscription(new_subscription("Quiet"))
        .await
        .expect("subscription");
    let mut connection = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(directory.path().join(crate::tests::DATABASE_FILE)),
    )
    .await
    .expect("connect");
    let kept = crate::subscription_store::SUBSCRIPTION_RUNS_KEPT;
    let now = Timestamp::now();
    for (id, runs) in [(subscription.id, kept + 20), (quiet.id, 3)] {
        for age in 1..=runs {
            sqlx::query(
                "INSERT INTO subscription_runs (id, subscription_id, started_at) VALUES (?, ?, ?)",
            )
            .bind(rd_core::SubscriptionRunId::new().to_string())
            .bind(id.to_string())
            .bind(now - SignedDuration::from_mins(age))
            .execute(&mut connection)
            .await
            .expect("run");
        }
    }
    for id in [subscription.id, quiet.id] {
        database
            .finish_subscription_run(
                id,
                now,
                crate::PollResult {
                    found: 7,
                    accepted: 0,
                    skipped: 7,
                    error: None,
                    next_run_at: now,
                    consecutive_failures: 0,
                    etag: None,
                    last_modified: None,
                },
            )
            .await
            .expect("finish");
    }

    let runs = database
        .subscription_runs(subscription.id, kept + 50)
        .await
        .expect("runs");
    assert_eq!(runs.len() as i64, kept);
    assert_eq!(runs[0].found, 7, "the newest one stays");
    let quiet = database
        .subscription_runs(quiet.id, 50)
        .await
        .expect("runs");
    assert_eq!(quiet.len(), 4, "fewer than are kept lose nothing");
}
