//! The poll loop with an indexer source (RD-1150-05): a deep poll is archived whole, and a
//! `429` becomes a pause rather than a failure.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Utc};
use rd_core::SubscriptionKind;
use rd_subscription::{DiscoveredItem, PollOutcome, RateLimited, SourceAdapter};

use super::{adapters::rate_limited, tests::service_with};

/// An indexer whose poll paged to its bound: every entry new, every page full.
struct DeepIndexer;

#[async_trait::async_trait]
impl SourceAdapter for DeepIndexer {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::Indexer
    }

    async fn poll(&self, _subscription: &rd_core::Subscription) -> anyhow::Result<PollOutcome> {
        let items = (0..rd_subscription::MAX_INDEXER_ITEMS)
            .map(|number| -> anyhow::Result<DiscoveredItem> {
                let mut item = DiscoveredItem::new(
                    format!("Release.{number}"),
                    format!("https://indexer.test/get/{number}.nzb").parse()?,
                );
                item.source_id = Some(format!("id-{number}"));
                Ok(item)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(PollOutcome {
            items,
            ..PollOutcome::default()
        })
    }
}

fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2030, 1, 15, 12, 0, 0)
        .single()
        .expect("time")
}

fn indexer() -> rd_db::NewSubscription {
    rd_db::NewSubscription {
        name: "Busy category".to_owned(),
        url: "https://indexer.test/api".parse().expect("url"),
        kind: SubscriptionKind::Indexer,
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
        indexer_search: rd_core::IndexerSearch::default(),
        git_release: rd_core::GitReleaseOptions::default(),
        secret_ref: None,
    }
}

/// The next poll stops where it meets this archive, so an entry this poll brought and did not
/// archive would never come back. The run's counts are also what the interface reads a gap
/// from: found above a first poll's depth, and every entry archived as new.
#[tokio::test]
async fn a_deep_indexer_poll_is_archived_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let adapters: Vec<Arc<dyn SourceAdapter>> = vec![Arc::new(DeepIndexer)];
    let (service, database) =
        service_with(directory.path(), adapters, Arc::new(Mutex::new(start()))).await;
    let created = database
        .create_subscription(indexer())
        .await
        .expect("subscription");

    service.poll_now(created.id).await.expect("poll");
    let runs = database
        .subscription_runs(created.id, 1)
        .await
        .expect("runs");
    let run = runs.first().expect("one run");
    let all = u32::try_from(rd_subscription::MAX_INDEXER_ITEMS).expect("count");
    assert_eq!(run.error, None);
    assert_eq!(run.found, all);
    assert_eq!(run.accepted + run.skipped, all, "{run:?}");
    service.shutdown();
}

fn too_many_requests(retry_after_seconds: Option<u64>) -> anyhow::Error {
    rd_http::HttpDownloadError::Failure(rd_core::Failure::new(
        rd_core::FailureKind::RateLimited {
            retry_after_seconds,
        },
        "HTTP 429 Too Many Requests",
    ))
    .into()
}

#[test]
fn a_429_is_a_pause_until_its_retry_after() {
    let now = start();

    let named = rate_limited(too_many_requests(Some(600)), now);
    assert_eq!(
        named.downcast_ref::<RateLimited>(),
        Some(&RateLimited {
            until: now + Duration::minutes(10)
        })
    );
    // No time named: a minute, like every other refusal that names none.
    let unnamed = rate_limited(too_many_requests(None), now);
    assert_eq!(
        unnamed.downcast_ref::<RateLimited>(),
        Some(&RateLimited {
            until: now + Duration::minutes(1)
        })
    );
    // A day at most, whatever the server asks for.
    let endless = rate_limited(too_many_requests(Some(u64::MAX)), now);
    assert_eq!(
        endless.downcast_ref::<RateLimited>(),
        Some(&RateLimited {
            until: now + Duration::days(1)
        })
    );
}

#[test]
fn any_other_fetch_error_stays_what_it_was() {
    let error = rate_limited(anyhow::anyhow!("HTTP 503 Service Unavailable"), start());

    assert!(error.downcast_ref::<RateLimited>().is_none());
    assert_eq!(error.to_string(), "HTTP 503 Service Unavailable");
}
