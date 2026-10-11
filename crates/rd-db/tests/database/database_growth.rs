//! What keeps the database from growing with use (RD-1240-35): change notices are broadcast but
//! not persisted, an old skipped or dismissed subscription item keeps only its key — and stays
//! recognised by it — and the free pages go back to the file system without a rewrite.

use chrono::{Duration, Utc};
use rd_core::{EventKind, SubscriptionItemState};
use rd_db::{Database, NewIndexer, NewSubscription, NewSubscriptionItem};
use sqlx::{Connection, SqliteConnection};
use tempfile::TempDir;

async fn open(directory: &TempDir) -> (Database, SqliteConnection) {
    let path = directory.path().join("growth.sqlite");
    let database = Database::open(&path).await.expect("database");
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("connect");
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&mut connection)
        .await
        .expect("busy timeout");
    (database, connection)
}

async fn count(connection: &mut SqliteConnection, statement: &'static str) -> i64 {
    sqlx::query_scalar(statement)
        .fetch_one(connection)
        .await
        .expect("count")
}

fn subscription() -> NewSubscription {
    NewSubscription {
        name: "Indexer".to_owned(),
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
        script_arguments: Vec::new(),
        indexer_search: rd_core::IndexerSearch::default(),
        git_release: rd_core::GitReleaseOptions::default(),
        secret_ref: None,
    }
}

fn item(key: &str, state: SubscriptionItemState) -> NewSubscriptionItem {
    NewSubscriptionItem {
        item_key: key.to_owned(),
        title: format!("Release {key}"),
        url: format!("https://indexer.test/get/{key}.nzb")
            .parse()
            .expect("url"),
        published_at: None,
        duration_seconds: None,
        state,
        reason: None,
        source_category: None,
        media_type: None,
        attributes: std::collections::BTreeMap::new(),
        password: None,
    }
}

/// Moves every item of the archive `days` into the past.
async fn age_items(connection: &mut SqliteConnection, days: i64) {
    sqlx::query("UPDATE subscription_items SET discovered_at = ?")
        .bind(Utc::now() - Duration::days(days))
        .execute(connection)
        .await
        .expect("age items");
}

#[tokio::test]
async fn a_usenet_change_is_broadcast_but_not_persisted_and_other_kinds_still_are() {
    let directory = TempDir::new().expect("tempdir");
    let (database, mut connection) = open(&directory).await;
    let mut events = database.subscribe();

    database
        .create_indexer(NewIndexer {
            name: "Example".to_owned(),
            url: "https://indexer.test/api".parse().expect("url"),
            secret_ref: None,
            categories: Vec::new(),
            enabled: true,
            list_style: rd_core::IndexerListStyle::Compact,
        })
        .await
        .expect("indexer");
    database
        .create_subscription(subscription())
        .await
        .expect("subscription");

    // Live exactly as before: both reach a subscriber, in order.
    assert_eq!(
        events.try_recv().expect("usenet").kind,
        EventKind::UsenetChanged
    );
    assert_eq!(
        events.try_recv().expect("subscription").kind,
        EventKind::SubscriptionChanged
    );
    // The table keeps only the one that is not a change notice.
    let usenet = count(
        &mut connection,
        "SELECT COUNT(*) FROM events WHERE kind = 'usenet_changed'",
    )
    .await;
    let subscription = count(
        &mut connection,
        "SELECT COUNT(*) FROM events WHERE kind = 'subscription_changed'",
    )
    .await;
    assert_eq!((usenet, subscription), (0, 1));
}

#[tokio::test]
async fn a_compacted_item_keeps_its_key_and_a_poll_that_lists_it_again_brings_nothing_back() {
    let directory = TempDir::new().expect("tempdir");
    let (database, mut connection) = open(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    database
        .record_subscription_items(
            created.id,
            vec![
                item("id:skipped", SubscriptionItemState::Skipped),
                item("id:dismissed", SubscriptionItemState::Dismissed),
                item("id:pending", SubscriptionItemState::Pending),
                item("id:queued", SubscriptionItemState::Queued),
            ],
        )
        .await
        .expect("record");
    age_items(&mut connection, 120).await;

    let before = Utc::now() - Duration::days(30);
    let storage = database
        .database_storage(Some(before))
        .await
        .expect("storage");
    assert_eq!((storage.item_rows, storage.compactable_items), (4, 2));
    assert_eq!(
        database
            .compact_subscription_items(before)
            .await
            .expect("compact"),
        2
    );

    // The settled two left the history; what is undecided or queued is untouched.
    let page = database
        .subscription_item_page(created.id, None, 50, 0)
        .await
        .expect("page");
    let mut kept: Vec<_> = page
        .items
        .iter()
        .map(|item| item.item_key.as_str())
        .collect();
    kept.sort_unstable();
    assert_eq!(kept, ["id:pending", "id:queued"]);
    let keys = count(
        &mut connection,
        "SELECT COUNT(*) FROM subscription_item_keys",
    )
    .await;
    assert_eq!(keys, 2);

    // The once-only guarantee holds through the key: re-listed, nothing is new.
    let again = database
        .record_subscription_items(
            created.id,
            vec![
                item("id:skipped", SubscriptionItemState::Pending),
                item("id:dismissed", SubscriptionItemState::Pending),
                item("id:fresh", SubscriptionItemState::Pending),
            ],
        )
        .await
        .expect("poll again");
    let fresh: Vec<_> = again.iter().map(|item| item.item_key.as_str()).collect();
    assert_eq!(fresh, ["id:fresh"]);
    for key in ["id:skipped", "id:dismissed"] {
        assert!(
            database
                .subscription_knows_item(created.id, key)
                .await
                .expect("knows"),
            "{key}"
        );
    }
    // A second pass finds nothing more to move.
    assert_eq!(
        database
            .compact_subscription_items(before)
            .await
            .expect("again"),
        0
    );
}

#[tokio::test]
async fn items_inside_the_retention_and_a_retention_of_zero_keep_their_rows() {
    let directory = TempDir::new().expect("tempdir");
    let (database, mut connection) = open(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    database
        .record_subscription_items(
            created.id,
            vec![item("id:recent", SubscriptionItemState::Skipped)],
        )
        .await
        .expect("record");
    age_items(&mut connection, 10).await;

    let before = Utc::now() - Duration::days(30);
    assert_eq!(
        database
            .compact_subscription_items(before)
            .await
            .expect("compact"),
        0
    );
    let storage = database.database_storage(None).await.expect("storage");
    assert_eq!((storage.item_rows, storage.compactable_items), (1, 0));
}

#[tokio::test]
async fn clearing_the_history_or_deleting_the_subscription_forgets_the_keys() {
    let directory = TempDir::new().expect("tempdir");
    let (database, mut connection) = open(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    database
        .record_subscription_items(
            created.id,
            vec![
                item("id:a", SubscriptionItemState::Skipped),
                item("id:b", SubscriptionItemState::Skipped),
            ],
        )
        .await
        .expect("record");
    age_items(&mut connection, 120).await;
    database
        .compact_subscription_items(Utc::now() - Duration::days(30))
        .await
        .expect("compact");

    database
        .clear_subscription_history(created.id)
        .await
        .expect("clear");
    let keys = count(
        &mut connection,
        "SELECT COUNT(*) FROM subscription_item_keys",
    )
    .await;
    assert_eq!(keys, 0, "cleared like the rows they stand for");

    database
        .record_subscription_items(
            created.id,
            vec![item("id:c", SubscriptionItemState::Skipped)],
        )
        .await
        .expect("record");
    age_items(&mut connection, 120).await;
    database
        .compact_subscription_items(Utc::now() - Duration::days(30))
        .await
        .expect("compact");
    database
        .delete_subscription(created.id)
        .await
        .expect("delete");
    let keys = count(
        &mut connection,
        "SELECT COUNT(*) FROM subscription_item_keys",
    )
    .await;
    assert_eq!(keys, 0);
}

#[tokio::test]
async fn a_new_file_is_incremental_and_hands_its_free_pages_back() {
    let directory = TempDir::new().expect("tempdir");
    let (database, mut connection) = open(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    // Enough settled rows with long titles to fill a few hundred pages.
    let items = (0..3_000)
        .map(|index| {
            let mut entry = item(&format!("id:{index}"), SubscriptionItemState::Skipped);
            entry.title = "x".repeat(1_000);
            entry
        })
        .collect();
    database
        .record_subscription_items(created.id, items)
        .await
        .expect("record");
    age_items(&mut connection, 120).await;

    let full = database.database_storage(None).await.expect("storage");
    assert!(full.incremental, "{full:?}");
    assert!(full.item_bytes > 3_000_000, "{full:?}");
    assert_eq!(
        database
            .compact_subscription_items(Utc::now() - Duration::days(30))
            .await
            .expect("compact"),
        3_000
    );
    let compacted = database.database_storage(None).await.expect("storage");
    assert!(compacted.free_bytes > 2_000_000, "{compacted:?}");

    let reclaimed = database.reclaim_free_pages().await.expect("reclaim");
    let after = database.database_storage(None).await.expect("storage");
    assert!(reclaimed > 2_000_000, "{reclaimed}");
    assert_eq!(after.free_bytes, 0, "{after:?}");
    assert_eq!(after.file_bytes, compacted.file_bytes - reclaimed);
}

#[tokio::test]
async fn a_rewrite_makes_an_older_file_incremental() {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("older.sqlite");
    // A file created before RD-1240-35: `auto_vacuum` was never asked for.
    {
        let mut older = SqliteConnection::connect(&format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .expect("create");
        sqlx::query("CREATE TABLE placeholder (id INTEGER PRIMARY KEY)")
            .execute(&mut older)
            .await
            .expect("table");
        older.close().await.expect("close");
    }
    let database = Database::open(&path).await.expect("database");
    assert!(
        !database
            .database_storage(None)
            .await
            .expect("before")
            .incremental
    );
    database.rewrite().await.expect("rewrite");
    assert!(
        database
            .database_storage(None)
            .await
            .expect("after")
            .incremental
    );
}
