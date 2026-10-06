//! What the store keeps for a git-release subscription (RD-190-13): its options, the
//! validators an options edit must drop, and a first poll that failed priming nothing.

use chrono::Utc;
use rd_db::{Database, NewSubscription, PollResult};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("subscriptions.sqlite"))
        .await
        .expect("database")
}

fn subscription() -> NewSubscription {
    NewSubscription {
        name: "Tool releases".to_owned(),
        url: "https://github.com/example/tool".parse().expect("url"),
        kind: rd_core::SubscriptionKind::GitRelease,
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
        git_release: rd_core::GitReleaseOptions {
            platforms: vec![rd_core::GitPlatform::Linux],
            architectures: vec![rd_core::GitArchitecture::X86_64],
            ..rd_core::GitReleaseOptions::default()
        },
        secret_ref: None,
    }
}

fn result(error: Option<&str>, etag: Option<&str>) -> PollResult {
    PollResult {
        found: 0,
        accepted: 0,
        skipped: 0,
        error: error.map(ToOwned::to_owned),
        next_run_at: Utc::now(),
        consecutive_failures: u32::from(error.is_some()),
        etag: etag.map(ToOwned::to_owned),
        last_modified: None,
    }
}

#[tokio::test]
async fn the_options_are_stored_and_read_back_with_the_kind() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    let stored = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("stored");
    assert_eq!(stored.kind, rd_core::SubscriptionKind::GitRelease);
    assert_eq!(stored.git_release, subscription().git_release);
}

/// A first poll stopped by a rate limit or a network error decided nothing: the next one
/// still applies the backlog policy, or it would take the whole release history as new.
#[tokio::test]
async fn a_failed_first_poll_leaves_the_backlog_decision_to_the_next() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");

    database
        .finish_subscription_run(created.id, Utc::now(), result(Some("HTTP 503"), None))
        .await
        .expect("failed run");
    let after_failure = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("stored");
    assert!(!after_failure.primed);

    database
        .finish_subscription_run(created.id, Utc::now(), result(None, None))
        .await
        .expect("run");
    let after_success = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("stored");
    assert!(after_success.primed);
}

/// An unchanged release list answers `304`, so validators kept across an options edit would
/// hide the assets the new options select until the repository publishes something else.
#[tokio::test]
async fn an_options_edit_drops_the_validators_and_another_edit_keeps_them() {
    let directory = TempDir::new().expect("tempdir");
    let database = database(&directory).await;
    let created = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    database
        .finish_subscription_run(created.id, Utc::now(), result(None, Some("\"v1\"")))
        .await
        .expect("run");

    let mut renamed = subscription();
    renamed.name = "Renamed".to_owned();
    let (kept, _) = database
        .update_subscription(created.id, renamed)
        .await
        .expect("rename");
    assert_eq!(kept.etag.as_deref(), Some("\"v1\""));

    let mut widened = subscription();
    widened.git_release.prereleases = true;
    let (dropped, _) = database
        .update_subscription(created.id, widened)
        .await
        .expect("options edit");
    assert_eq!(dropped.etag, None);
}
