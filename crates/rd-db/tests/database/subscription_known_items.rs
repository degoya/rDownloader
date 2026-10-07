//! Where an indexer poll meets what its subscription already has (RD-1150-05).
//!
//! The poll asks the archive about the last entry of each result page and stops paging at the
//! first one it knows. The answer has to be the archive's own: every state counts, and another
//! subscription's archive does not.

use rd_db::{Database, NewSubscription, NewSubscriptionItem};
use tempfile::TempDir;

fn subscription(name: &str) -> NewSubscription {
    NewSubscription {
        name: name.to_owned(),
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

fn item(key: &str, state: rd_core::SubscriptionItemState) -> NewSubscriptionItem {
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

#[tokio::test]
async fn the_archive_knows_its_own_items_in_every_state_and_no_others() {
    let directory = TempDir::new().expect("tempdir");
    let database = Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database");
    let first = database
        .create_subscription(subscription("Indexer A"))
        .await
        .expect("first");
    let second = database
        .create_subscription(subscription("Indexer B"))
        .await
        .expect("second");
    database
        .record_subscription_items(
            first.id,
            vec![
                item("id:pending", rd_core::SubscriptionItemState::Pending),
                item("id:skipped", rd_core::SubscriptionItemState::Skipped),
            ],
        )
        .await
        .expect("record");

    for key in ["id:pending", "id:skipped"] {
        assert!(
            database
                .subscription_knows_item(first.id, key)
                .await
                .expect("lookup"),
            "{key}"
        );
    }
    assert!(
        !database
            .subscription_knows_item(first.id, "id:new")
            .await
            .expect("lookup")
    );
    assert!(
        !database
            .subscription_knows_item(second.id, "id:pending")
            .await
            .expect("lookup"),
        "another subscription's archive is not this one's"
    );
}
