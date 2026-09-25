//! Newznab and Torznab indexers as a subscription source (RD-080-11).
//!
//! Both protocols are the same shape: a query string against a base URL, answered with an
//! RSS document whose items carry `<newznab:attr>` / `<torznab:attr>` metadata. So this is
//! not a second parser — it builds the query, hands the response to the feed parser, and
//! reads the extra attributes off the items.
//!
//! **The API key is the whole security surface here.** It travels in a query parameter,
//! which means it is one careless log line away from ending up in a file. Two rules follow
//! and both are enforced here rather than trusted to callers:
//!
//! * the key is fetched from the vault at the moment the request is built and never stored
//!   anywhere else;
//! * every address that could reach a log, an error message or the UI goes through
//!   [`redact_query`] first, which is the same treatment `rd-core` gives a signed URL.

use async_trait::async_trait;
use std::{collections::HashSet, sync::Arc};
use url::Url;

use rd_core::{Subscription, SubscriptionKind};

use crate::{
    adapter::{DiscoveredItem, PollOutcome, SourceAdapter},
    feed::parse_feed,
    feed_adapter::FeedFetcher,
};

/// Most results asked for in one query. Indexers cap this themselves, usually at 100.
pub const DEFAULT_LIMIT: u32 = 100;
/// Most result pages one poll asks an indexer for.
pub const MAX_PAGES: u32 = 5;

/// Resolves a vault reference into the secret it stands for.
///
/// A trait so this crate never links the secret store and a test can supply a key without
/// one; the reference itself is opaque here and is never inspected.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve(&self, reference: &str) -> anyhow::Result<String>;
}

/// Builds the query address for one indexer poll.
///
/// `t=search` with no `q` is the "recent items" query every Newznab and Torznab server
/// answers, which is exactly what a subscription wants: whatever is new since last time.
/// Any query the user put in the subscription's own address is preserved, so a saved search
/// with categories and a search term keeps working.
///
/// **The subscription's own title filter is deliberately not turned into a `q` (RD-106-10).**
/// The filter language is a case-insensitive substring and, written in slashes, a regular
/// expression; `q` is whatever the indexer's own tokenizer makes of the words it is given, and
/// the two do not agree. Deriving one from the other would silently drop hits the filter
/// accepts — a worse failure than the one it would fix, because it looks exactly the same from
/// the outside. The consequence is real and stays: results arrive `limit` at a time and the
/// filter runs afterwards. Five pages make older local matches available without turning one
/// poll into an unbounded crawl; the interface names that 500-result boundary when it is hit.
pub fn build_query(
    base: &Url,
    api_key: &str,
    limit: u32,
    categories: &[String],
) -> anyhow::Result<Url> {
    build_page_query(base, api_key, limit, 0, categories)
}

/// Builds one page of an indexer query.
///
/// `limit` and `offset` belong to the poller rather than to a copied saved-search URL: keeping a
/// stale offset would ask for the same page forever, and keeping a tiny limit would make the
/// five-request ceiling arbitrarily small. Every other parameter remains exactly as supplied.
pub fn build_page_query(
    base: &Url,
    api_key: &str,
    limit: u32,
    offset: u32,
    categories: &[String],
) -> anyhow::Result<Url> {
    let mut url = base.clone();
    // Existing parameters win: a subscription URL is usually copied out of the indexer's own
    // "RSS feed" button and already carries `t`, `cat` and often `q`.
    let existing: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let has = |name: &str| {
        existing
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(name))
    };
    {
        let mut query = url.query_pairs_mut();
        query.clear();
        for (key, value) in &existing {
            // The key is replaced rather than appended: a stale one copied out of a browser
            // would otherwise be sent alongside the stored one.
            if key.eq_ignore_ascii_case("apikey")
                || key.eq_ignore_ascii_case("api_key")
                || key.eq_ignore_ascii_case("limit")
                || key.eq_ignore_ascii_case("offset")
            {
                continue;
            }
            query.append_pair(key, value);
        }
        if !has("t") {
            query.append_pair("t", "search");
        }
        query.append_pair("limit", &limit.to_string());
        query.append_pair("offset", &offset.to_string());
        // Asks for the `<newznab:attr>` block that carries size and category.
        if !has("extended") {
            query.append_pair("extended", "1");
        }
        // Only when the address does not already say which categories it wants: a subscription
        // URL is usually copied out of the indexer's own RSS button, and one that already
        // carries `cat` is a saved search whose author meant it. Without this the categories a
        // subscription had chosen only ever sorted the results afterwards, so an indexer was
        // asked for everything and most of it was thrown away.
        if !has("cat") && !categories.is_empty() {
            query.append_pair("cat", &categories.join(","));
        }
        query.append_pair("apikey", api_key);
    }
    Ok(url)
}

/// Builds the `t=caps` address for one indexer.
///
/// Everything but the base and the key is dropped: a saved search's `cat` and `q` are not
/// only pointless for a capability request, some indexers reject the combination.
pub fn build_caps_query(base: &Url, api_key: &str) -> anyhow::Result<Url> {
    let mut url = base.clone();
    {
        let mut query = url.query_pairs_mut();
        query.clear();
        query.append_pair("t", "caps");
        query.append_pair("apikey", api_key);
    }
    Ok(url)
}

/// The address with every credential-bearing parameter masked.
///
/// Used for *everything* that leaves this module as text — an error message, a log line, a
/// diagnostic. `rd_core::redact_url` already knows `apikey` and `api_key`, so this is one
/// call rather than a second list that could fall out of step with the first.
#[must_use]
pub fn redact_query(url: &Url) -> String {
    rd_core::redact_url(url)
}

/// Polls Newznab and Torznab indexers.
pub struct IndexerAdapter {
    fetcher: Arc<dyn FeedFetcher>,
    secrets: Arc<dyn SecretResolver>,
}

impl IndexerAdapter {
    #[must_use]
    pub fn new(fetcher: Arc<dyn FeedFetcher>, secrets: Arc<dyn SecretResolver>) -> Self {
        Self { fetcher, secrets }
    }
}

#[async_trait]
impl SourceAdapter for IndexerAdapter {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::Indexer
    }

    async fn poll(&self, subscription: &Subscription) -> anyhow::Result<PollOutcome> {
        let Some(reference) = subscription.secret_ref.as_deref() else {
            anyhow::bail!("indexer subscription has no API key");
        };
        let api_key = self.secrets.resolve(reference).await?;
        let mut items = Vec::new();
        let mut keys = HashSet::new();
        for page in 0..MAX_PAGES {
            let offset = page.saturating_mul(DEFAULT_LIMIT);
            let url = build_page_query(
                &subscription.url,
                &api_key,
                DEFAULT_LIMIT,
                offset,
                &subscription.source_categories,
            )?;

            // Conditional headers are pointless for a search query — the answer changes with
            // the clock — so neither validator is sent and none is stored.
            let fetched = self
                .fetcher
                .fetch(&url, None, None)
                .await
                // The address carries the key, so an error that quoted it would defeat the
                // point of storing it encrypted. Masked before it becomes a message.
                .map_err(|error| anyhow::anyhow!("{} ({})", error, redact_query(&url)))?;
            let Some(body) = fetched.body else {
                if page == 0 {
                    return Ok(PollOutcome {
                        not_modified: true,
                        ..PollOutcome::default()
                    });
                }
                anyhow::bail!("indexer returned no body for result page {}", page + 1);
            };
            // Errors come back as a document, not a status: `<error code="100"
            // description="Incorrect user credentials"/>`. Reported before parsing, or a wrong
            // key looks like an indexer with nothing new.
            if let Some(message) = indexer_error(&body) {
                anyhow::bail!("indexer refused the query: {message}");
            }

            let base = fetched.final_url.as_ref().unwrap_or(&subscription.url);
            let feed = parse_feed(&body, base)?;
            let announced = feed.items.len();
            let mut usable = 0usize;
            for item in feed.items {
                let Some(url) = item.download_url().cloned() else {
                    continue;
                };
                // `extended=1` is sent for exactly this: the attribute block travels with the
                // search answer, so what the indexer knows about a release costs no second
                // request. Filtered before it is kept — see `attributes::retain`.
                let kept = crate::retain_attributes(&item.attributes, item.size_bytes);
                let discovered = DiscoveredItem {
                    source_id: item.id.clone(),
                    title: item.title.clone(),
                    url,
                    published_at: item.published_at,
                    duration_seconds: None,
                    language: item.language.clone(),
                    height: None,
                    published_raw: item.published_raw.clone(),
                    // Newznab reports it as an attribute; the subscription's map turns it
                    // into one of our categories.
                    source_category: item.categories.first().cloned(),
                    media_type: item.enclosure_type.clone(),
                    attributes: kept.attributes,
                    // An indexer names its own id for every release, which is a stronger
                    // signal than any name; release recognition is RD-110-21's alone.
                    release_key: None,
                    password: kept.password,
                };
                usable += 1;
                if keys.insert(crate::key_of(&discovered)) {
                    items.push(discovered);
                }
            }
            // An entry with neither an enclosure nor a link cannot be downloaded, so it is
            // dropped here rather than stored. Saying so keeps "the indexer returned nothing"
            // apart from "the indexer returned entries this build cannot use".
            if usable < announced {
                tracing::warn!(
                    announced,
                    usable,
                    page = page + 1,
                    "indexer entries without a download address were dropped"
                );
            }
            if announced < DEFAULT_LIMIT as usize {
                break;
            }
        }
        Ok(PollOutcome {
            items,
            etag: None,
            last_modified: None,
            not_modified: false,
        })
    }
}

/// Extracts a Newznab/Torznab `<error …>` description, if the document is one.
///
/// Deliberately a string scan rather than a parse: the error document is tiny, and running
/// the full parser first would mean a malformed *error* was reported as a malformed feed.
#[must_use]
pub fn indexer_error(body: &str) -> Option<String> {
    let start = body.find("<error")?;
    let rest = &body[start..];
    let end = rest.find('>')?;
    let tag = &rest[..end];
    let description = extract_attribute(tag, "description");
    let code = extract_attribute(tag, "code");
    match (description, code) {
        (Some(description), Some(code)) => Some(format!("{description} (code {code})")),
        (Some(description), None) => Some(description),
        (None, Some(code)) => Some(format!("code {code}")),
        (None, None) => Some("unspecified error".to_owned()),
    }
}

fn extract_attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_LIMIT, IndexerAdapter, MAX_PAGES, SecretResolver, build_page_query, build_query,
        indexer_error, redact_query,
    };
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
            id: rd_core::SubscriptionId::new(),
            name: "Indexer".to_owned(),
            source_categories: Vec::new(),
            url: base("https://indexer.test/api"),
            kind: rd_core::SubscriptionKind::Indexer,
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
            secret_ref: Some("vault://key".to_owned()),
            has_secret: true,
            every_release: false,
            view: rd_core::SubscriptionView::List,
            autoplay: false,
            card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
            schedule: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
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

    #[test]
    fn a_bare_address_becomes_a_recent_items_query() {
        let url = build_query(
            &base("https://indexer.test/api"),
            "SECRET",
            DEFAULT_LIMIT,
            &[],
        )
        .expect("query");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert!(pairs.contains(&("t".to_owned(), "search".to_owned())));
        assert!(pairs.contains(&("extended".to_owned(), "1".to_owned())));
        assert!(pairs.contains(&("limit".to_owned(), "100".to_owned())));
        assert!(pairs.contains(&("offset".to_owned(), "0".to_owned())));
        assert!(pairs.contains(&("apikey".to_owned(), "SECRET".to_owned())));
    }

    #[test]
    fn a_saved_search_keeps_its_own_parameters() {
        // A subscription address is usually copied out of the indexer's own RSS button and
        // already carries the query that makes it worth subscribing to.
        let url = build_query(
            &base("https://indexer.test/api?t=tvsearch&cat=5030,5040&q=example"),
            "SECRET",
            DEFAULT_LIMIT,
            &[],
        )
        .expect("query");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert!(pairs.contains(&("t".to_owned(), "tvsearch".to_owned())));
        assert!(pairs.contains(&("cat".to_owned(), "5030,5040".to_owned())));
        assert!(pairs.contains(&("q".to_owned(), "example".to_owned())));
        // Not overridden with `search`.
        assert_eq!(pairs.iter().filter(|(key, _)| key == "t").count(), 1);
    }

    #[test]
    fn pagination_replaces_stale_limit_and_offset_only() {
        let url = build_page_query(
            &base("https://indexer.test/api?t=tvsearch&q=show&limit=20&offset=900"),
            "SECRET",
            DEFAULT_LIMIT,
            200,
            &[],
        )
        .expect("query");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert!(pairs.contains(&("q".to_owned(), "show".to_owned())));
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "limit")
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>(),
            ["100"]
        );
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "offset")
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>(),
            ["200"]
        );
    }

    /// Records the address it was asked for, so what actually reaches an indexer can be
    /// asserted on rather than inferred from the builder.
    struct RecordingFetcher(std::sync::Mutex<Option<Url>>);

    #[async_trait]
    impl FeedFetcher for RecordingFetcher {
        async fn fetch(
            &self,
            url: &Url,
            _etag: Option<&str>,
            _last_modified: Option<&str>,
        ) -> anyhow::Result<FetchedFeed> {
            *self.0.lock().expect("lock") = Some(url.clone());
            Ok(FetchedFeed {
                body: Some(EXTENDED_RESULT.to_owned()),
                etag: None,
                last_modified: None,
                final_url: None,
            })
        }
    }

    /// RD-106-10: the title filter stays a local decision, and that is a choice, not an
    /// oversight. Sending it as `q` would hand a substring or a regular expression to an
    /// indexer's word tokenizer, which answers with a different set — including fewer hits
    /// than the filter would have accepted. The cost of keeping it local is the page
    /// boundary below, which the interface names instead.
    #[tokio::test]
    async fn a_title_filter_is_never_sent_as_a_search_term() {
        let fetcher = Arc::new(RecordingFetcher(std::sync::Mutex::new(None)));
        let mut subscription = indexer_subscription();
        subscription.filters.title_contains = vec![
            "german".to_owned(),
            "/^s0\\d/".to_owned(),
            "1080p".to_owned(),
        ];
        IndexerAdapter::new(fetcher.clone(), Arc::new(StaticKey))
            .poll(&subscription)
            .await
            .expect("poll");

        let url = fetcher
            .0
            .lock()
            .expect("lock")
            .clone()
            .expect("the adapter should have asked for something");
        assert!(
            url.query_pairs().all(|(key, _)| key != "q"),
            "a filter pattern reached the indexer as a search term: {url}"
        );
        // The page the filter is applied to is the one this limit asks for.
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "limit" && value == DEFAULT_LIMIT.to_string()),
            "{url}"
        );
    }

    #[test]
    fn a_key_pasted_into_the_address_is_replaced_by_the_stored_one() {
        // Otherwise a stale key copied out of a browser would be sent alongside the real
        // one, and which of the two the server honours is anybody's guess.
        let url = build_query(
            &base("https://indexer.test/api?apikey=STALE&t=search"),
            "STORED",
            DEFAULT_LIMIT,
            &[],
        )
        .expect("query");
        let keys: Vec<String> = url
            .query_pairs()
            .filter(|(key, _)| key == "apikey")
            .map(|(_, value)| value.into_owned())
            .collect();
        assert_eq!(keys, vec!["STORED".to_owned()]);
    }

    #[test]
    fn the_api_key_never_survives_redaction() {
        // The one property this module exists to guarantee.
        let url = build_query(
            &base("https://indexer.test/api?t=search"),
            "super-secret-key",
            DEFAULT_LIMIT,
            &[],
        )
        .expect("query");
        assert!(url.as_str().contains("super-secret-key"));
        let masked = redact_query(&url);
        assert!(!masked.contains("super-secret-key"), "{masked}");
        // The rest of the address survives, or a diagnostic would be useless.
        assert!(masked.contains("indexer.test"));
        assert!(masked.contains("t=search"));
    }

    #[test]
    fn an_indexer_error_document_is_recognised() {
        // A wrong key answers 200 with this; without the check it would look like an
        // indexer that simply had nothing new.
        let body =
            r#"<?xml version="1.0"?><error code="100" description="Incorrect user credentials"/>"#;
        assert_eq!(
            indexer_error(body).as_deref(),
            Some("Incorrect user credentials (code 100)")
        );
    }

    #[test]
    fn an_ordinary_result_document_is_not_an_error() {
        let body = r#"<rss><channel><item><title>x</title></item></channel></rss>"#;
        assert!(indexer_error(body).is_none());
    }

    #[test]
    fn an_error_without_a_description_still_reports_something() {
        assert_eq!(
            indexer_error(r#"<error code="910"/>"#).as_deref(),
            Some("code 910")
        );
    }

    /// The reported case: a subscription that wants one category was pulling the whole feed.
    ///
    /// The category map only sorted what had already arrived; nothing ever narrowed the ask.
    #[test]
    fn chosen_categories_are_asked_for_rather_than_filtered_afterwards() {
        let url = build_query(
            &base("https://indexer.test/api"),
            "SECRET",
            DEFAULT_LIMIT,
            &["3010".to_owned(), "3040".to_owned()],
        )
        .expect("query");
        let pairs: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert!(pairs.contains(&("cat".to_owned(), "3010,3040".to_owned())));
    }

    #[test]
    fn without_a_choice_the_query_asks_for_everything_as_before() {
        let url = build_query(
            &base("https://indexer.test/api"),
            "SECRET",
            DEFAULT_LIMIT,
            &[],
        )
        .expect("query");
        assert!(!url.query_pairs().any(|(key, _)| key == "cat"));
    }

    /// A saved search pasted out of the indexer's own RSS button already says what it wants,
    /// and its author meant it. The stored choice must not overwrite that.
    #[test]
    fn an_address_that_already_names_categories_keeps_its_own() {
        let url = build_query(
            &base("https://indexer.test/api?t=tvsearch&cat=5030,5040"),
            "SECRET",
            DEFAULT_LIMIT,
            &["3010".to_owned()],
        )
        .expect("query");
        let cats: Vec<String> = url
            .query_pairs()
            .filter(|(key, _)| key == "cat")
            .map(|(_, value)| value.into_owned())
            .collect();
        assert_eq!(cats, vec!["5030,5040".to_owned()]);
    }
}
