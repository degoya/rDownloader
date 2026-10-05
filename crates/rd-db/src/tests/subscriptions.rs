//! Subscriptions (RD-080-07).

use chrono::{Duration, Utc};

use crate::Database;

fn new_subscription(name: &str) -> crate::NewSubscription {
    crate::NewSubscription {
        source_categories: Vec::new(),
        name: name.to_owned(),
        url: "https://example.test/c/channel".parse().expect("url"),
        kind: rd_core::SubscriptionKind::Media,
        enabled: true,
        mode: rd_core::SubscriptionMode::Review,
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters::default(),
        backlog: rd_core::BacklogPolicy::FromNow,
        category_map: Vec::new(),
        every_release: false,
        view: rd_core::SubscriptionView::List,
        autoplay: false,
        card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
        schedule: None,
        script_arguments: Vec::new(),
        indexer_search: rd_core::IndexerSearch::default(),
        git_release: rd_core::GitReleaseOptions::default(),
        secret_ref: None,
    }
}

fn new_item(key: &str) -> crate::NewSubscriptionItem {
    crate::NewSubscriptionItem {
        item_key: key.to_owned(),
        title: format!("Item {key}"),
        url: format!("https://example.test/watch?v={key}")
            .parse()
            .expect("url"),
        published_at: Some(Utc::now()),
        duration_seconds: Some(600),
        state: rd_core::SubscriptionItemState::Pending,
        reason: None,
        source_category: None,
        media_type: None,
        attributes: std::collections::BTreeMap::new(),
        password: None,
    }
}

#[tokio::test]
async fn an_item_is_archived_once_however_often_a_poll_repeats_it() {
    // The whole once-only guarantee. A feed that re-lists the same entry, an overlapping
    // poll and a poll interrupted before it finished must all produce one row and one
    // download, which is why the UNIQUE index does the work instead of a read-then-write.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("subscription");

    let first = database
        .record_subscription_items(subscription.id, vec![new_item("a"), new_item("b")])
        .await
        .expect("first poll");
    assert_eq!(first.len(), 2);

    // The same two, plus one that is genuinely new.
    let second = database
        .record_subscription_items(
            subscription.id,
            vec![new_item("b"), new_item("a"), new_item("c")],
        )
        .await
        .expect("second poll");
    assert_eq!(
        second.len(),
        1,
        "only the new item should be returned, got {second:?}"
    );
    assert_eq!(second[0].item_key, "c");

    let archived = database
        .subscription_item_page(subscription.id, None, 100, 0)
        .await
        .expect("items")
        .items;
    assert_eq!(archived.len(), 3);
}

/// What an indexer said about a hit survives the round trip (RD-101-17).
#[tokio::test]
async fn an_items_attributes_and_password_are_stored_and_read_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    // Archive passwords live in the vault (RD-190-04).
    database
        .install_file_vault(directory.path().join("secrets"))
        .await
        .expect("vault");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");

    let mut item = new_item("a");
    item.attributes = [
        (
            "coverurl".to_owned(),
            "https://indexer.test/c.jpg".to_owned(),
        ),
        ("imdbscore".to_owned(), "7.8".to_owned()),
        ("size".to_owned(), "4509715660".to_owned()),
    ]
    .into_iter()
    .collect();
    item.password = Some("hunter2".to_owned());

    let created = database
        .record_subscription_items(subscription.id, vec![item])
        .await
        .expect("poll");
    assert_eq!(created.len(), 1);

    let archived = database
        .subscription_item_page(subscription.id, None, 100, 0)
        .await
        .expect("items")
        .items;
    let stored = archived.first().expect("one item");
    assert_eq!(
        stored.attributes.get("coverurl").map(String::as_str),
        Some("https://indexer.test/c.jpg")
    );
    assert_eq!(
        stored.attributes.get("imdbscore").map(String::as_str),
        Some("7.8")
    );
    assert_eq!(stored.password.as_deref(), Some("hunter2"));
}

#[tokio::test]
async fn subscription_item_pages_reach_past_two_hundred_with_stable_totals() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-pages.sqlite"))
        .await
        .expect("database");
    let mut input = new_subscription("Indexer");
    input.kind = rd_core::SubscriptionKind::Indexer;
    let subscription = database
        .create_subscription(input)
        .await
        .expect("subscription");
    let items = (0..205)
        .map(|number| new_item(&format!("item-{number:03}")))
        .collect();
    database
        .record_subscription_items(subscription.id, items)
        .await
        .expect("items");

    let first = database
        .subscription_item_page(
            subscription.id,
            Some(rd_core::SubscriptionItemState::Pending),
            50,
            0,
        )
        .await
        .expect("first page");
    let last = database
        .subscription_item_page(
            subscription.id,
            Some(rd_core::SubscriptionItemState::Pending),
            50,
            200,
        )
        .await
        .expect("last page");
    let repeated = database
        .subscription_item_page(
            subscription.id,
            Some(rd_core::SubscriptionItemState::Pending),
            50,
            200,
        )
        .await
        .expect("same last page");

    assert_eq!(first.items.len(), 50);
    assert_eq!(last.items.len(), 5);
    assert_eq!(last.total, 205);
    assert_eq!(last.counts.pending, 205);
    assert_eq!(last.run_total, 0);
    assert_eq!(
        last.items.iter().map(|item| item.id).collect::<Vec<_>>(),
        repeated
            .items
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>()
    );
    assert!(
        first
            .items
            .iter()
            .all(|first| last.items.iter().all(|last| first.id != last.id))
    );

    let summary = database
        .subscription_review_summary()
        .await
        .expect("summary");
    assert_eq!(summary.pending_total, 205);
    assert_eq!(summary.subscriptions.len(), 1);
    assert_eq!(summary.subscriptions[0].pending, 205);
}

#[tokio::test]
async fn a_pending_bulk_snapshot_does_not_capture_items_that_arrive_later() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-bulk.sqlite"))
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
    let snapshot = database
        .pending_subscription_item_ids(subscription.id)
        .await
        .expect("snapshot");
    database
        .record_subscription_items(subscription.id, vec![new_item("later")])
        .await
        .expect("later item");
    assert_eq!(
        database
            .set_pending_subscription_items_state(
                snapshot,
                rd_core::SubscriptionItemState::Dismissed,
            )
            .await
            .expect("bulk state"),
        2
    );

    let page = database
        .subscription_item_page(subscription.id, None, 50, 0)
        .await
        .expect("page");
    assert_eq!(page.counts.dismissed, 2);
    assert_eq!(page.counts.pending, 1);
    assert_eq!(
        page.items
            .iter()
            .find(|item| item.item_key == "later")
            .expect("later item")
            .state,
        rd_core::SubscriptionItemState::Pending
    );
}

#[tokio::test]
async fn clearing_subscription_history_preserves_pending_items_only() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-history.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");
    let created = database
        .record_subscription_items(
            subscription.id,
            vec![
                new_item("pending"),
                new_item("queued"),
                new_item("dismissed"),
                new_item("skipped"),
            ],
        )
        .await
        .expect("items");
    for (key, state) in [
        ("queued", rd_core::SubscriptionItemState::Queued),
        ("dismissed", rd_core::SubscriptionItemState::Dismissed),
        ("skipped", rd_core::SubscriptionItemState::Skipped),
    ] {
        let id = created
            .iter()
            .find(|item| item.item_key == key)
            .expect("item")
            .id;
        database
            .set_subscription_item_state(id, state)
            .await
            .expect("state");
    }
    for _ in 0..2 {
        database
            .finish_subscription_run(
                subscription.id,
                Utc::now(),
                crate::PollResult {
                    found: 4,
                    accepted: 1,
                    skipped: 3,
                    error: None,
                    next_run_at: Utc::now() + Duration::hours(1),
                    consecutive_failures: 0,
                    etag: None,
                    last_modified: None,
                },
            )
            .await
            .expect("run");
    }

    let removed = database
        .clear_subscription_history(subscription.id)
        .await
        .expect("clear");
    assert_eq!(removed.deleted_items, 3);
    assert_eq!(removed.deleted_runs, 2);
    let remaining = database
        .subscription_item_page(subscription.id, None, 50, 0)
        .await
        .expect("remaining");
    assert_eq!(remaining.total, 1);
    assert_eq!(remaining.items[0].item_key, "pending");
    assert_eq!(remaining.counts.pending, 1);
    assert_eq!(remaining.run_total, 0);
}

/// A later poll that answers without the extended block must not erase what is known.
///
/// The upsert refreshes a still-pending row, so a plain `excluded.attributes_json` would
/// replace a full attribute set with nothing the first time an indexer omitted it.
#[tokio::test]
async fn a_repoll_without_attributes_keeps_the_ones_already_stored() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    // Archive passwords live in the vault (RD-190-04).
    database
        .install_file_vault(directory.path().join("secrets"))
        .await
        .expect("vault");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");

    let mut rich = new_item("a");
    rich.attributes = [("imdbscore".to_owned(), "7.8".to_owned())]
        .into_iter()
        .collect();
    rich.password = Some("hunter2".to_owned());
    database
        .record_subscription_items(subscription.id, vec![rich])
        .await
        .expect("first poll");

    // The same item, this time with nothing said about it.
    let bare = new_item("a");
    assert!(bare.attributes.is_empty());
    database
        .record_subscription_items(subscription.id, vec![bare])
        .await
        .expect("second poll");

    let archived = database
        .subscription_item_page(subscription.id, None, 100, 0)
        .await
        .expect("items")
        .items;
    let stored = archived.first().expect("one item");
    assert_eq!(
        stored.attributes.get("imdbscore").map(String::as_str),
        Some("7.8"),
        "a silent poll must not blank the details"
    );
    assert_eq!(stored.password.as_deref(), Some("hunter2"));
}

#[tokio::test]
async fn two_subscriptions_do_not_share_an_archive() {
    // The key is only unique *within* a subscription: the same video legitimately appears
    // in a channel and in a playlist, and each has to decide for itself.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let first = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("first");
    let second = database
        .create_subscription(new_subscription("Playlist"))
        .await
        .expect("second");

    assert_eq!(
        database
            .record_subscription_items(first.id, vec![new_item("a")])
            .await
            .expect("first")
            .len(),
        1
    );
    assert_eq!(
        database
            .record_subscription_items(second.id, vec![new_item("a")])
            .await
            .expect("second")
            .len(),
        1
    );
}

#[tokio::test]
async fn the_archive_survives_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("subscriptions.sqlite");
    let id = {
        let database = Database::open(&path).await.expect("database");
        let subscription = database
            .create_subscription(new_subscription("Channel"))
            .await
            .expect("subscription");
        database
            .record_subscription_items(subscription.id, vec![new_item("a")])
            .await
            .expect("poll");
        subscription.id
    };

    let database = Database::open(&path).await.expect("reopen");
    let repeated = database
        .record_subscription_items(id, vec![new_item("a")])
        .await
        .expect("poll after restart");
    assert!(
        repeated.is_empty(),
        "a restart must not make an archived item new again"
    );
}

#[tokio::test]
async fn a_new_subscription_is_due_at_once_and_a_finished_poll_pushes_it_out() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("subscription");
    // No next_run_at: the backlog decision is made and shown immediately rather than an
    // interval from now.
    let due = database.due_subscriptions(Utc::now()).await.expect("due");
    assert_eq!(due.len(), 1);
    assert!(!due[0].primed);

    let next = Utc::now() + Duration::hours(1);
    database
        .finish_subscription_run(
            subscription.id,
            Utc::now(),
            crate::PollResult {
                found: 3,
                accepted: 1,
                skipped: 2,
                error: None,
                next_run_at: next,
                consecutive_failures: 0,
                etag: None,
                last_modified: None,
            },
        )
        .await
        .expect("finish");

    assert!(
        database
            .due_subscriptions(Utc::now())
            .await
            .expect("due")
            .is_empty()
    );
    let stored = database
        .subscription(subscription.id)
        .await
        .expect("get")
        .expect("exists");
    // Primed, and never unprimed: re-applying the backlog cutoff later would discard
    // everything published since the subscription was switched on.
    assert!(stored.primed);
    let runs = database
        .subscription_runs(subscription.id, 10)
        .await
        .expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0].found, runs[0].accepted, runs[0].skipped),
        (3, 1, 2)
    );
}

#[tokio::test]
async fn a_scheduled_script_subscription_is_armed_once_and_a_new_schedule_clears_its_time() {
    // RD-130-19. The script travels as a `script:` address, the expression as a column, and
    // arming only ever fills an empty next run.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let mut input = new_subscription("Daily links");
    input.kind = rd_core::SubscriptionKind::Script;
    input.url = "script:daily-links.sh".parse().expect("url");
    input.schedule = Some("0 6 * * *".to_owned());
    let created = database
        .create_subscription(input.clone())
        .await
        .expect("subscription");
    assert_eq!(created.kind, rd_core::SubscriptionKind::Script);
    assert_eq!(created.script_name(), Some("daily-links.sh"));
    assert_eq!(created.schedule.as_deref(), Some("0 6 * * *"));
    assert!(created.next_run_at.is_none());

    // Whole seconds, so the comparison below is about the row and not about precision.
    let six =
        chrono::DateTime::from_timestamp(Utc::now().timestamp() + 3 * 3_600, 0).expect("time");
    assert!(
        database
            .arm_subscription(created.id, six)
            .await
            .expect("arm")
    );
    // Armed already: a second arm, say from a tick racing a finished run, changes nothing.
    assert!(
        !database
            .arm_subscription(created.id, six + Duration::hours(1))
            .await
            .expect("arm again")
    );
    let stored = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(stored.next_run_at, Some(six));
    assert_eq!(stored.kind, rd_core::SubscriptionKind::Script);
    assert!(
        database
            .due_subscriptions(Utc::now())
            .await
            .expect("due")
            .is_empty()
    );

    // The same expression again keeps the time; another one clears it for the poller to arm.
    let (kept, _) = database
        .update_subscription(created.id, input.clone())
        .await
        .expect("update");
    assert_eq!(kept.next_run_at, Some(six));
    input.schedule = Some("30 7 * * 1-5".to_owned());
    let (changed, _) = database
        .update_subscription(created.id, input)
        .await
        .expect("update");
    assert_eq!(changed.schedule.as_deref(), Some("30 7 * * 1-5"));
    assert!(changed.next_run_at.is_none());
}

#[tokio::test]
async fn a_script_subscription_keeps_its_arguments_one_by_one() {
    // RD-150-08. Each argument is stored as it was given -- spaces, quotes and shell
    // operators included -- and an edit replaces the whole list.
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let mut input = new_subscription("Daily links");
    input.kind = rd_core::SubscriptionKind::Script;
    input.url = "script:daily-links.sh".parse().expect("url");
    let plain = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("subscription");
    assert!(plain.script_arguments.is_empty());

    input.script_arguments = vec![
        "--since".to_owned(),
        "two words".to_owned(),
        "a&b;c".to_owned(),
        "it's \"quoted\"".to_owned(),
        String::new(),
    ];
    let created = database
        .create_subscription(input.clone())
        .await
        .expect("subscription");
    assert_eq!(created.script_arguments, input.script_arguments);
    let stored = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("exists");
    assert_eq!(stored.script_arguments, input.script_arguments);

    input.script_arguments = vec!["--full".to_owned()];
    let (changed, _) = database
        .update_subscription(created.id, input)
        .await
        .expect("update");
    assert_eq!(changed.script_arguments, ["--full"]);
}

#[tokio::test]
async fn deleting_a_subscription_takes_its_archive_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Channel"))
        .await
        .expect("subscription");
    database
        .record_subscription_items(subscription.id, vec![new_item("a")])
        .await
        .expect("poll");

    database
        .delete_subscription(subscription.id)
        .await
        .expect("delete");
    assert!(
        database
            .subscription(subscription.id)
            .await
            .expect("get")
            .is_none()
    );
    assert!(
        database
            .subscription_item_page(subscription.id, None, 100, 0)
            .await
            .expect("items")
            .items
            .is_empty()
    );
}

/// A repeat poll corrects an item nobody has decided on, and leaves decided ones alone.
///
/// The address and the declared media type belong to the feed. An item archived before either
/// was read correctly is otherwise stuck with what was stored then — which is exactly the
/// situation an upgrade leaves behind, and it is not something a person can repair by hand.
#[tokio::test]
async fn a_repeat_poll_refreshes_undecided_items_only() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::Database::open(&directory.path().join("test.sqlite3"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");

    let stale = crate::NewSubscriptionItem {
        url: "https://indexer.test/api?t=get&amp;id=a"
            .parse()
            .expect("url"),
        media_type: None,
        ..new_item("a")
    };
    let decided = crate::NewSubscriptionItem {
        url: "https://indexer.test/api?t=get&amp;id=b"
            .parse()
            .expect("url"),
        media_type: None,
        ..new_item("b")
    };
    let created = database
        .record_subscription_items(subscription.id, vec![stale, decided])
        .await
        .expect("first poll");
    assert_eq!(created.len(), 2);
    let decided_id = created
        .iter()
        .find(|item| item.item_key == "b")
        .expect("b")
        .id;
    database
        .set_subscription_item_state(decided_id, rd_core::SubscriptionItemState::Queued)
        .await
        .expect("decide");

    // The same entries, read correctly this time.
    let corrected = |key: &str| crate::NewSubscriptionItem {
        url: format!("https://indexer.test/api?t=get&id={key}")
            .parse()
            .expect("url"),
        media_type: Some("application/x-nzb".to_owned()),
        ..new_item(key)
    };
    let second = database
        .record_subscription_items(subscription.id, vec![corrected("a"), corrected("b")])
        .await
        .expect("second poll");

    assert!(
        second.is_empty(),
        "a refreshed item is not a discovery: {second:?}"
    );
    let archived = database
        .subscription_item_page(subscription.id, None, 100, 0)
        .await
        .expect("items")
        .items;
    let item = |key: &str| {
        archived
            .iter()
            .find(|item| item.item_key == key)
            .unwrap_or_else(|| panic!("{key} missing"))
    };
    assert_eq!(
        item("a").url.as_str(),
        "https://indexer.test/api?t=get&id=a",
        "the undecided one is corrected"
    );
    assert_eq!(item("a").media_type.as_deref(), Some("application/x-nzb"));
    assert_eq!(
        item("b").url.as_str(),
        "https://indexer.test/api?t=get&amp;id=b",
        "a decision is never undone by a re-listing"
    );
    assert_eq!(item("b").media_type, None);
    assert_eq!(item("b").state, rd_core::SubscriptionItemState::Queued);
}
