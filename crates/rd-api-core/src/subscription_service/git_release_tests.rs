//! The poll loop with a git-release source (RD-190-13): a rate limit is waited out, and a
//! release file archived but not handed over survives a stop without being lost or doubled.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use chrono::{DateTime, Duration, TimeZone, Utc};
use rd_core::{SubscriptionItemState, SubscriptionKind};
use rd_subscription::{DiscoveredItem, PollOutcome, RateLimited, SourceAdapter};

use super::tests::service_with;

/// A repository with one release file, or one that refuses for its rate limit.
#[derive(Default)]
struct FakeReleases {
    polls: AtomicUsize,
    limited_until: Mutex<Option<DateTime<Utc>>>,
    refused: AtomicBool,
}

#[async_trait::async_trait]
impl SourceAdapter for FakeReleases {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::GitRelease
    }

    async fn poll(&self, _subscription: &rd_core::Subscription) -> anyhow::Result<PollOutcome> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if let Some(until) = *self.limited_until.lock().expect("limit") {
            return Err(RateLimited { until }.into());
        }
        let mut item = DiscoveredItem::new(
            "tool-1.2.0-linux-x86_64.tar.gz".to_owned(),
            "https://github.com/example/tool/releases/download/v1.2.0/tool-1.2.0-linux-x86_64.tar.gz"
                .parse()?,
        );
        item.source_id = Some("github:release:3001:asset:9101".to_owned());
        item.published_at = Some(start());
        if self.refused.load(Ordering::SeqCst) {
            item.refused = Some(rd_core::FilterReason::AssetNotWanted);
        }
        Ok(PollOutcome {
            items: vec![item],
            ..PollOutcome::default()
        })
    }
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2030, 1, 15, 12, 0, 0)
        .single()
        .expect("time")
}

fn releases(mode: rd_core::SubscriptionMode) -> rd_db::NewSubscription {
    rd_db::NewSubscription {
        name: "Tool releases".to_owned(),
        url: "https://github.com/example/tool".parse().expect("url"),
        kind: SubscriptionKind::GitRelease,
        enabled: true,
        mode,
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters::default(),
        // Everything published from a day before the clock on is new.
        backlog: rd_core::BacklogPolicy::Since(start() - Duration::days(1)),
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

#[tokio::test]
async fn a_rate_limit_is_waited_out_without_counting_as_a_failure_or_priming() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = Arc::new(FakeReleases::default());
    let until = start() + Duration::minutes(40);
    *source.limited_until.lock().expect("limit") = Some(until);
    let adapters: Vec<Arc<dyn SourceAdapter>> = vec![source.clone()];
    let (service, database) =
        service_with(directory.path(), adapters, Arc::new(Mutex::new(start()))).await;
    let created = database
        .create_subscription(releases(rd_core::SubscriptionMode::Review))
        .await
        .expect("subscription");

    service.poll_now(created.id).await.expect("poll");
    let stored = database
        .subscription(created.id)
        .await
        .expect("get")
        .expect("stored");
    assert_eq!(stored.next_run_at, Some(until), "waits for the named time");
    assert_eq!(stored.consecutive_failures, 0, "a rate limit is no failure");
    assert!(
        !stored.primed,
        "nothing was learned, so nothing was decided"
    );
    assert!(
        stored
            .last_error
            .as_deref()
            .is_some_and(|error| error.contains("rate limit")),
        "{:?}",
        stored.last_error
    );
    service.shutdown();
}

#[tokio::test]
async fn a_file_the_options_refuse_is_archived_as_skipped_with_the_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = Arc::new(FakeReleases::default());
    source.refused.store(true, Ordering::SeqCst);
    let adapters: Vec<Arc<dyn SourceAdapter>> = vec![source.clone()];
    let (service, database) =
        service_with(directory.path(), adapters, Arc::new(Mutex::new(start()))).await;
    let created = database
        .create_subscription(releases(rd_core::SubscriptionMode::AutoQueue))
        .await
        .expect("subscription");

    service.poll_now(created.id).await.expect("poll");
    let items = database
        .subscription_item_page(created.id, None, 10, 0)
        .await
        .expect("items")
        .items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].state, SubscriptionItemState::Skipped);
    assert_eq!(items[0].reason, Some(rd_core::FilterReason::AssetNotWanted));
    assert!(
        database
            .list_collector_batches()
            .await
            .expect("batches")
            .is_empty()
    );
    service.shutdown();
}

/// `subscription.after_items_archived`: the release file is archived, the LinkGrabber never
/// got it. The next start's poll must neither hand it over a second time nor lose it.
#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_release_file_archived_but_not_handed_over_stays_for_review_after_a_stop() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = Arc::new(FakeReleases::default());
    let adapters: Vec<Arc<dyn SourceAdapter>> = vec![source.clone()];
    let (service, database) =
        service_with(directory.path(), adapters, Arc::new(Mutex::new(start()))).await;
    let created = database
        .create_subscription(releases(rd_core::SubscriptionMode::AutoQueue))
        .await
        .expect("subscription");

    {
        let guard = rd_core::failpoint::FailpointGuard::once("subscription.after_items_archived");
        service.poll_now(created.id).await.expect("poll");
        assert!(guard.fired(), "the crash point was never reached");
    }
    let pending = |database: rd_db::Database| async move {
        database
            .subscription_item_page(created.id, None, 10, 0)
            .await
            .expect("items")
            .items
    };
    let items = pending(database.clone()).await;
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].state,
        SubscriptionItemState::Pending,
        "archived, never claimed as queued"
    );
    assert!(
        database
            .list_collector_batches()
            .await
            .expect("batches")
            .is_empty()
    );

    // The next start polls again: the file is already archived, so nothing is handed over a
    // second time, and it is still there to be queued from the review list.
    service.poll_now(created.id).await.expect("poll");
    assert_eq!(source.polls.load(Ordering::SeqCst), 2);
    let items = pending(database.clone()).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].state, SubscriptionItemState::Pending);
    assert!(
        database
            .list_collector_batches()
            .await
            .expect("batches")
            .is_empty()
    );
    service.shutdown();
}
