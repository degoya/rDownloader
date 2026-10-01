//! The indexers a person defines once (RD-180-19): stored, edited and removed with their key
//! reference handed back for the vault, and a subscription's search parameters (RD-180-20).

use rd_core::EventKind;
use rd_db::{Database, NewIndexer, StoreErrorKind, store_kind};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("indexers.sqlite"))
        .await
        .expect("database")
}

fn indexer(name: &str, secret_ref: Option<&str>) -> NewIndexer {
    NewIndexer {
        name: name.to_owned(),
        url: "https://indexer.test/api".parse().expect("url"),
        secret_ref: secret_ref.map(ToOwned::to_owned),
        categories: vec!["2000".to_owned(), "5040".to_owned()],
        enabled: true,
    }
}

#[tokio::test]
async fn an_indexer_round_trips_and_never_serialises_its_key_reference() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let mut events = database.subscribe();

    let created = database
        .create_indexer(indexer("Example", Some("vault://first")))
        .await
        .expect("create");
    assert!(created.has_secret);
    assert_eq!(created.categories, ["2000", "5040"]);
    let event = events.try_recv().expect("an event");
    assert_eq!(event.kind, EventKind::UsenetChanged);
    assert_eq!(event.payload["resource"], "indexer");

    let listed = database.list_indexers().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].secret_ref.as_deref(), Some("vault://first"));
    let json = serde_json::to_string(&listed[0]).expect("serialise");
    assert!(!json.contains("vault://"), "{json}");
    assert!(json.contains("\"has_secret\":true"), "{json}");
}

#[tokio::test]
async fn an_edit_without_a_key_keeps_it_and_a_new_key_hands_the_old_one_back() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_indexer(indexer("Example", Some("vault://first")))
        .await
        .expect("create");

    let mut edit = indexer("Renamed", None);
    edit.enabled = false;
    let (kept, orphan) = database
        .update_indexer(created.id, edit)
        .await
        .expect("update");
    assert_eq!(kept.name, "Renamed");
    assert!(!kept.enabled);
    assert_eq!(kept.secret_ref.as_deref(), Some("vault://first"));
    assert_eq!(orphan, None);

    let (replaced, orphan) = database
        .update_indexer(created.id, indexer("Renamed", Some("vault://second")))
        .await
        .expect("update");
    assert_eq!(replaced.secret_ref.as_deref(), Some("vault://second"));
    assert_eq!(orphan.as_deref(), Some("vault://first"));

    let removed = database.delete_indexer(created.id).await.expect("delete");
    assert_eq!(removed.as_deref(), Some("vault://second"));
    assert!(database.indexer(created.id).await.expect("get").is_none());
}

#[tokio::test]
async fn a_second_indexer_of_the_same_name_is_a_duplicate_and_an_unknown_one_is_not_found() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    database
        .create_indexer(indexer("Example", Some("vault://first")))
        .await
        .expect("create");
    let error = database
        .create_indexer(indexer("Example", Some("vault://other")))
        .await
        .expect_err("duplicate");
    assert_eq!(store_kind(&error), Some(StoreErrorKind::Duplicate));

    let missing = rd_core::IndexerId::new();
    let error = database
        .update_indexer(missing, indexer("Other", None))
        .await
        .expect_err("missing");
    assert_eq!(store_kind(&error), Some(StoreErrorKind::NotFound));
    let error = database.delete_indexer(missing).await.expect_err("missing");
    assert_eq!(store_kind(&error), Some(StoreErrorKind::NotFound));
}

/// RD-180-20: the search a subscription sends is stored and read back as it was given.
#[tokio::test]
async fn a_subscription_keeps_its_search_parameters() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let search = rd_core::IndexerSearch {
        query: Some("some show !german".to_owned()),
        max_age_days: Some(14),
        hide_passworded: true,
        pretime: Some(2),
    };
    let created = database
        .create_subscription(rd_db::NewSubscription {
            name: "Indexer".to_owned(),
            url: "https://indexer.test/api".parse().expect("url"),
            kind: rd_core::SubscriptionKind::Indexer,
            enabled: true,
            mode: rd_core::SubscriptionMode::Review,
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            interval_seconds: 3_600,
            filters: rd_core::SubscriptionFilters::default(),
            backlog: rd_core::BacklogPolicy::FromNow,
            category_map: Vec::new(),
            source_categories: Vec::new(),
            every_release: false,
            view: rd_core::SubscriptionView::List,
            autoplay: false,
            card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
            schedule: None,
            script_arguments: Vec::new(),
            indexer_search: search.clone(),
            secret_ref: None,
        })
        .await
        .expect("create");
    assert_eq!(created.indexer_search, search);
    let stored = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("stored");
    assert_eq!(stored.indexer_search, search);
}
