use super::{DEFAULT_LIMIT, IndexerAdapter, MAX_PAGES, SecretResolver};
use crate::adapter::SourceAdapter;
use crate::feed_adapter::{FeedFetcher, FetchedFeed};
use async_trait::async_trait;
use std::sync::Arc;
use url::Url;

fn base(input: &str) -> Url {
    input.parse().expect("url")
}

/// One search answer with the `extended=1` attribute block a real indexer sends.
const EXTENDED_RESULT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <rss xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
      <channel>
        <item>
          <title>Some.Movie.2024.1080p.BluRay.x265</title>
          <guid>abc123</guid>
          <enclosure url="https://indexer.test/getnzb/abc123.nzb" length="4509715660"
                     type="application/x-nzb"/>
          <newznab:attr name="category" value="2040"/>
          <newznab:attr name="coverurl" value="https://indexer.test/covers/movies/551.jpg"/>
          <newznab:attr name="imdb" value="0111161"/>
          <newznab:attr name="imdbscore" value="7.8"/>
          <newznab:attr name="resolution" value="1080p"/>
          <newznab:attr name="video" value="x265"/>
          <newznab:attr name="grabs" value="12"/>
          <newznab:attr name="password" value="0"/>
        </item>
      </channel>
    </rss>"#;

struct StaticFetcher(&'static str);

#[async_trait]
impl FeedFetcher for StaticFetcher {
    async fn fetch(
        &self,
        _url: &Url,
        _etag: Option<&str>,
        _last_modified: Option<&str>,
    ) -> anyhow::Result<FetchedFeed> {
        Ok(FetchedFeed {
            body: Some(self.0.to_owned()),
            etag: None,
            last_modified: None,
            final_url: None,
        })
    }
}

struct StaticKey;

#[async_trait]
impl SecretResolver for StaticKey {
    async fn resolve(&self, _reference: &str) -> anyhow::Result<String> {
        Ok("SECRET".to_owned())
    }
}

fn indexer_subscription() -> rd_core::Subscription {
    rd_core::Subscription {
        secret_ref: Some("vault://key".to_owned()),
        has_secret: true,
        ..crate::test_support::subscription(
            "Indexer",
            rd_core::SubscriptionKind::Indexer,
            "https://indexer.test/api",
        )
    }
}

async fn poll_fixture(body: &'static str) -> crate::adapter::PollOutcome {
    IndexerAdapter::new(Arc::new(StaticFetcher(body)), Arc::new(StaticKey))
        .poll(&indexer_subscription())
        .await
        .expect("poll")
}

fn result_page(start: usize, count: usize) -> String {
    let items = (start..start + count)
        .map(|number| {
            format!(
                "<item><title>Release {number}</title><guid>id-{number}</guid>\
                     <enclosure url=\"https://indexer.test/get/{number}.nzb\" \
                     type=\"application/x-nzb\"/></item>"
            )
        })
        .collect::<String>();
    format!("<?xml version=\"1.0\"?><rss><channel>{items}</channel></rss>")
}

struct PagingFetcher {
    counts: Vec<usize>,
    requested: std::sync::Mutex<Vec<Url>>,
    fail_offset: Option<u32>,
    overlap: bool,
}

#[async_trait]
impl FeedFetcher for PagingFetcher {
    async fn fetch(
        &self,
        url: &Url,
        _etag: Option<&str>,
        _last_modified: Option<&str>,
    ) -> anyhow::Result<FetchedFeed> {
        self.requested.lock().expect("lock").push(url.clone());
        let offset = url
            .query_pairs()
            .find(|(key, _)| key == "offset")
            .and_then(|(_, value)| value.parse::<u32>().ok())
            .expect("offset");
        if self.fail_offset == Some(offset) {
            anyhow::bail!("next page failed");
        }
        let page = usize::try_from(offset / DEFAULT_LIMIT).expect("page");
        let count = self.counts.get(page).copied().unwrap_or_default();
        let mut start = usize::try_from(offset).expect("offset fits");
        if self.overlap && page > 0 {
            start = start.saturating_sub(1);
        }
        Ok(FetchedFeed {
            body: Some(result_page(start, count)),
            etag: None,
            last_modified: None,
            final_url: None,
        })
    }
}

#[tokio::test]
async fn polling_uses_offsets_and_stops_after_a_short_page() {
    let fetcher = Arc::new(PagingFetcher {
        counts: vec![100, 3],
        requested: std::sync::Mutex::new(Vec::new()),
        fail_offset: None,
        overlap: false,
    });
    let mut subscription = indexer_subscription();
    subscription.url = base("https://indexer.test/api?t=tvsearch&q=kept&cat=5040");
    let outcome = IndexerAdapter::new(fetcher.clone(), Arc::new(StaticKey))
        .poll(&subscription)
        .await
        .expect("poll");

    assert_eq!(outcome.items.len(), 103);
    let requested = fetcher.requested.lock().expect("lock");
    assert_eq!(requested.len(), 2);
    for (url, expected) in requested.iter().zip(["0", "100"]) {
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "offset" && value == expected)
        );
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "q" && value == "kept")
        );
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "cat" && value == "5040")
        );
    }
}

#[tokio::test]
async fn polling_stops_at_five_full_pages() {
    let fetcher = Arc::new(PagingFetcher {
        counts: vec![100; usize::try_from(MAX_PAGES).expect("page count")],
        requested: std::sync::Mutex::new(Vec::new()),
        fail_offset: None,
        overlap: false,
    });
    let outcome = IndexerAdapter::new(fetcher.clone(), Arc::new(StaticKey))
        .poll(&indexer_subscription())
        .await
        .expect("poll");

    assert_eq!(outcome.items.len(), 500);
    assert_eq!(fetcher.requested.lock().expect("lock").len(), 5);
}

#[tokio::test]
async fn duplicate_hits_across_page_boundaries_are_kept_once() {
    let fetcher = Arc::new(PagingFetcher {
        counts: vec![100, 2],
        requested: std::sync::Mutex::new(Vec::new()),
        fail_offset: None,
        overlap: true,
    });
    let outcome = IndexerAdapter::new(fetcher, Arc::new(StaticKey))
        .poll(&indexer_subscription())
        .await
        .expect("poll");

    assert_eq!(outcome.items.len(), 101);
}

#[tokio::test]
async fn a_failed_follow_up_page_fails_the_whole_poll() {
    let fetcher = Arc::new(PagingFetcher {
        counts: vec![100, 100],
        requested: std::sync::Mutex::new(Vec::new()),
        fail_offset: Some(100),
        overlap: false,
    });
    let error = IndexerAdapter::new(fetcher, Arc::new(StaticKey))
        .poll(&indexer_subscription())
        .await
        .expect_err("second page should fail");

    assert!(error.to_string().contains("next page failed"));
    assert!(!error.to_string().contains("SECRET"));
}

/// The reason `extended=1` is sent at all: the attribute block travels with the search
/// answer, so nothing downstream needs a second request to know what a release is.
#[tokio::test]
async fn the_extended_attribute_block_reaches_the_discovered_item() {
    let outcome = poll_fixture(EXTENDED_RESULT).await;
    let item = outcome.items.first().expect("one item");

    assert_eq!(
        item.attributes.get("coverurl").map(String::as_str),
        Some("https://indexer.test/covers/movies/551.jpg")
    );
    assert_eq!(
        item.attributes.get("imdbscore").map(String::as_str),
        Some("7.8")
    );
    assert_eq!(
        item.attributes.get("resolution").map(String::as_str),
        Some("1080p")
    );
    assert_eq!(
        item.attributes.get("video").map(String::as_str),
        Some("x265")
    );
    assert_eq!(item.attributes.get("grabs").map(String::as_str), Some("12"));
    // From the enclosure, so a hit that omits the `size` attribute still has one.
    assert_eq!(
        item.attributes.get("size").map(String::as_str),
        Some("4509715660")
    );
    // `password="0"` means "not protected" and is not worth a column of zeroes.
    assert!(!item.attributes.contains_key("password"));
    assert!(item.password.is_none());
    // The category still travels the typed route it always did.
    assert_eq!(item.source_category.as_deref(), Some("2040"));
}

/// An indexer that writes a real password where the specification wants a flag is the
/// only case where one can be taken — and it must not be served back to a client.
#[tokio::test]
async fn an_announced_archive_password_is_kept_apart_from_the_attributes() {
    const WITH_PASSWORD: &str = r#"<?xml version="1.0"?>
        <rss xmlns:newznab="http://www.newznab.com/DTD/2010/feeds/attributes/">
          <channel><item>
            <title>Some.Release</title><guid>p1</guid>
            <enclosure url="https://indexer.test/getnzb/p1.nzb" type="application/x-nzb"/>
            <newznab:attr name="password" value="hunter2"/>
          </item></channel>
        </rss>"#;

    let outcome = poll_fixture(WITH_PASSWORD).await;
    let item = outcome.items.first().expect("one item");
    assert_eq!(item.password.as_deref(), Some("hunter2"));
    // What the client sees says "protected" and nothing more.
    assert_eq!(
        item.attributes.get("password").map(String::as_str),
        Some("1")
    );
    assert!(
        !item
            .attributes
            .values()
            .any(|value| value.contains("hunter2")),
        "the secret must not survive in the attribute map"
    );
}

#[path = "indexer_query_tests.rs"]
mod query;
