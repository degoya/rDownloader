//! RSS, Atom and podcast subscriptions (RD-080-10).
//!
//! The HTTP half is a trait rather than a client, for the same reason the media adapter
//! takes a probe: it keeps this crate free of a transport and makes conditional requests,
//! `304` handling and caching testable from a fake instead of a server.

use async_trait::async_trait;
use std::sync::Arc;
use url::Url;

use rd_core::{Subscription, SubscriptionKind};

use crate::{
    adapter::{DiscoveredItem, PollOutcome, SourceAdapter},
    feed::parse_feed,
};

/// What a conditional fetch returned.
#[derive(Clone, Debug, Default)]
pub struct FetchedFeed {
    /// `None` when the server answered `304`.
    pub body: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// The address the body actually came from, so relative links resolve correctly.
    pub final_url: Option<Url>,
}

/// Fetches a feed document, conditionally.
#[async_trait]
pub trait FeedFetcher: Send + Sync {
    /// `etag` and `last_modified` are what the last poll stored; sending them back is what
    /// turns an unchanged feed into a `304` and a poll into almost no traffic at all.
    async fn fetch(
        &self,
        url: &Url,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> anyhow::Result<FetchedFeed>;
}

/// Polls RSS, Atom and podcast feeds.
pub struct FeedAdapter {
    fetcher: Arc<dyn FeedFetcher>,
}

impl FeedAdapter {
    #[must_use]
    pub fn new(fetcher: Arc<dyn FeedFetcher>) -> Self {
        Self { fetcher }
    }
}

#[async_trait]
impl SourceAdapter for FeedAdapter {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::Feed
    }

    async fn poll(&self, subscription: &Subscription) -> anyhow::Result<PollOutcome> {
        let fetched = self
            .fetcher
            .fetch(
                &subscription.url,
                subscription.etag.as_deref(),
                subscription.last_modified.as_deref(),
            )
            .await?;
        let Some(body) = fetched.body else {
            // Nothing changed. An empty item list here is the truth, not a failure, and the
            // caller must not read it as "the feed deleted everything" — the archive only
            // ever *adds*, so an empty poll changes nothing either way.
            return Ok(PollOutcome {
                items: Vec::new(),
                etag: fetched.etag,
                last_modified: fetched.last_modified,
                not_modified: true,
            });
        };
        let base = fetched.final_url.as_ref().unwrap_or(&subscription.url);
        let feed = parse_feed(&body, base)?;
        let items = feed
            .items
            .into_iter()
            .filter_map(|item| {
                // An entry with no address at all is not something that can be downloaded.
                let url = item.download_url()?.clone();
                Some(DiscoveredItem {
                    source_id: item.id.clone(),
                    title: item.title.clone(),
                    url,
                    published_at: item.published_at,
                    duration_seconds: item.duration_seconds,
                    language: item.language.clone(),
                    height: None,
                    published_raw: item.published_raw.clone(),
                    source_category: item.categories.first().cloned(),
                    media_type: item.enclosure_type.clone(),
                    // A plain feed carries no Newznab attribute block. The field stays empty
                    // rather than being invented from `<media:*>`, which is a separate job.
                    attributes: std::collections::BTreeMap::new(),
                    // Identity stays the feed's own id or its address: a feed entry is not a
                    // release page, and reading names for it is RD-110-21's business alone.
                    release_key: None,
                    password: None,
                })
            })
            .collect();
        Ok(PollOutcome {
            items,
            etag: fetched.etag,
            last_modified: fetched.last_modified,
            not_modified: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FeedAdapter, FeedFetcher, FetchedFeed};
    use crate::adapter::SourceAdapter;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};
    use url::Url;

    const RSS: &str = r#"<rss><channel>
        <item><title>One</title><guid>a</guid><enclosure url="https://cdn.test/a.mp3"/></item>
        <item><title>Two</title><guid>b</guid><link>https://example.test/b</link></item>
        <item><title>No address</title><guid>c</guid></item>
    </channel></rss>"#;

    /// Records what it was asked for, so the conditional headers can be asserted on.
    struct FakeFetcher {
        response: FetchedFeed,
        seen: Mutex<Vec<(Option<String>, Option<String>)>>,
    }

    #[async_trait]
    impl FeedFetcher for FakeFetcher {
        async fn fetch(
            &self,
            _url: &Url,
            etag: Option<&str>,
            last_modified: Option<&str>,
        ) -> anyhow::Result<FetchedFeed> {
            self.seen
                .lock()
                .expect("lock")
                .push((etag.map(str::to_owned), last_modified.map(str::to_owned)));
            Ok(self.response.clone())
        }
    }

    fn subscription() -> rd_core::Subscription {
        rd_core::Subscription {
            id: rd_core::SubscriptionId::new(),
            name: "Feed".to_owned(),
            source_categories: Vec::new(),
            url: "https://example.test/feed.xml".parse().expect("url"),
            kind: rd_core::SubscriptionKind::Feed,
            enabled: true,
            mode: rd_core::SubscriptionMode::Review,
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            interval_seconds: 3_600,
            filters: rd_core::SubscriptionFilters::default(),
            backlog: rd_core::BacklogPolicy::default(),
            category_map: Vec::new(),
            primed: true,
            last_run_at: None,
            next_run_at: None,
            consecutive_failures: 0,
            last_error: None,
            etag: None,
            last_modified: None,
            secret_ref: None,
            has_secret: false,
            every_release: false,
            view: rd_core::SubscriptionView::List,
            autoplay: false,
            card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
            schedule: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn a_feed_becomes_items_with_the_enclosure_preferred() {
        let fetcher = Arc::new(FakeFetcher {
            response: FetchedFeed {
                body: Some(RSS.to_owned()),
                etag: Some("\"v1\"".to_owned()),
                last_modified: None,
                final_url: None,
            },
            seen: Mutex::new(Vec::new()),
        });
        let adapter = FeedAdapter::new(fetcher);
        let outcome = adapter.poll(&subscription()).await.expect("poll");

        // The entry with no address at all is dropped: there is nothing to download.
        assert_eq!(outcome.items.len(), 2);
        assert_eq!(outcome.items[0].url.as_str(), "https://cdn.test/a.mp3");
        assert_eq!(outcome.items[1].url.as_str(), "https://example.test/b");
        assert_eq!(outcome.etag.as_deref(), Some("\"v1\""));
        assert!(!outcome.not_modified);
    }

    #[tokio::test]
    async fn the_stored_validators_are_sent_back_on_the_next_poll() {
        let fetcher = Arc::new(FakeFetcher {
            response: FetchedFeed {
                body: Some(RSS.to_owned()),
                ..FetchedFeed::default()
            },
            seen: Mutex::new(Vec::new()),
        });
        let adapter = FeedAdapter::new(Arc::clone(&fetcher) as Arc<dyn FeedFetcher>);
        let mut subscription = subscription();
        subscription.etag = Some("\"v1\"".to_owned());
        subscription.last_modified = Some("Wed, 04 Feb 2026 13:00:00 GMT".to_owned());
        adapter.poll(&subscription).await.expect("poll");

        let seen = fetcher.seen.lock().expect("lock");
        assert_eq!(
            seen[0],
            (
                Some("\"v1\"".to_owned()),
                Some("Wed, 04 Feb 2026 13:00:00 GMT".to_owned())
            ),
            "an unchanged feed should cost a 304, not a full download"
        );
    }

    #[tokio::test]
    async fn a_not_modified_response_is_an_empty_poll_and_not_a_failure() {
        let fetcher = Arc::new(FakeFetcher {
            response: FetchedFeed {
                body: None,
                etag: Some("\"v1\"".to_owned()),
                last_modified: None,
                final_url: None,
            },
            seen: Mutex::new(Vec::new()),
        });
        let adapter = FeedAdapter::new(fetcher);
        let outcome = adapter.poll(&subscription()).await.expect("poll");
        assert!(outcome.not_modified);
        assert!(outcome.items.is_empty());
        // The validator is carried forward rather than cleared, or every poll would be a
        // full fetch again.
        assert_eq!(outcome.etag.as_deref(), Some("\"v1\""));
    }

    #[tokio::test]
    async fn a_malformed_feed_fails_the_poll_rather_than_emptying_it() {
        // A parse failure that returned zero items would look exactly like a feed that had
        // been emptied, and the subscription would report a clean run.
        let fetcher = Arc::new(FakeFetcher {
            response: FetchedFeed {
                body: Some("<rss><channel><item><title>Half".to_owned()),
                ..FetchedFeed::default()
            },
            seen: Mutex::new(Vec::new()),
        });
        let adapter = FeedAdapter::new(fetcher);
        assert!(adapter.poll(&subscription()).await.is_err());
    }

    #[tokio::test]
    async fn relative_links_resolve_against_the_address_the_body_came_from() {
        let fetcher = Arc::new(FakeFetcher {
            response: FetchedFeed {
                body: Some(
                    "<rss><channel><item><title>x</title><link>/e/1</link></item></channel></rss>"
                        .to_owned(),
                ),
                final_url: Some(
                    "https://redirected.test/feeds/main.xml"
                        .parse()
                        .expect("url"),
                ),
                ..FetchedFeed::default()
            },
            seen: Mutex::new(Vec::new()),
        });
        let adapter = FeedAdapter::new(fetcher);
        let outcome = adapter.poll(&subscription()).await.expect("poll");
        assert_eq!(outcome.items[0].url.as_str(), "https://redirected.test/e/1");
    }
}
