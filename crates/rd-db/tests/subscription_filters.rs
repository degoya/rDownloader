//! An unreadable filter set fails its row instead of becoming an empty one.
//!
//! Empty filters mean "accept everything". A `filters_json` blob that fails to parse used to
//! fall back to exactly that, so a corrupt column silently turned a subscription that excluded
//! most of a feed into one that queued all of it. The row is refused instead, the way a corrupt
//! `url` or `id` in the same row already is.

use rd_db::{Database, NewSubscription};
use sqlx::{Connection, SqliteConnection};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database")
}

/// A subscription that keeps most of its feed out.
fn subscription() -> NewSubscription {
    NewSubscription {
        name: "My Indexer".to_owned(),
        url: "https://indexer.test/api".parse().expect("url"),
        kind: rd_core::SubscriptionKind::Indexer,
        enabled: true,
        mode: rd_core::SubscriptionMode::AutoQueue,
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters {
            title_excludes: vec!["sample".to_owned()],
            ..rd_core::SubscriptionFilters::default()
        },
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

/// Writes `value` into the `filters_json` column of every subscription, bypassing the writer:
/// disk damage and hand editing are the only ways this column ever becomes unparseable.
async fn corrupt_filters(directory: &TempDir, value: &str) {
    let path = directory.path().join("subscriptions.sqlite");
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("raw connection");
    sqlx::query("UPDATE subscriptions SET filters_json = ?")
        .bind(value)
        .execute(&mut connection)
        .await
        .expect("corrupt");
    connection.close().await.expect("close");
}

#[tokio::test]
async fn a_readable_filter_set_survives_the_round_trip() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .create_subscription(subscription())
        .await
        .expect("create");

    let listed = database.list_subscriptions().await.expect("list");

    assert_eq!(listed[0].filters.title_excludes, vec!["sample".to_owned()]);
}

#[tokio::test]
async fn an_unreadable_filter_set_fails_the_row_instead_of_accepting_everything() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("create");
    corrupt_filters(&directory, "{not json at all").await;

    assert!(
        database.list_subscriptions().await.is_err(),
        "a subscription whose filters cannot be read must not be handed out with none"
    );
    assert!(database.subscription(created.id).await.is_err());
    assert!(
        database
            .due_subscriptions(chrono::Utc::now())
            .await
            .is_err(),
        "above all it must not be polled: that is where the whole feed would be queued"
    );
}

/// A NULL column is not corruption — it predates the column and never filtered anything.
#[tokio::test]
async fn a_null_filter_column_still_reads_as_no_filters() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .create_subscription(subscription())
        .await
        .expect("create");
    let path = directory.path().join("subscriptions.sqlite");
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("raw connection");
    sqlx::query("UPDATE subscriptions SET filters_json = NULL")
        .execute(&mut connection)
        .await
        .expect("clear");
    connection.close().await.expect("close");

    let listed = database.list_subscriptions().await.expect("list");

    assert!(listed[0].filters.title_excludes.is_empty());
}
