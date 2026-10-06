//! The query an indexer is sent: address, saved search, categories, search term, and the key.

use super::{EXTENDED_RESULT, StaticKey, base, indexer_subscription};
use crate::adapter::SourceAdapter;
use crate::feed_adapter::{FeedFetcher, FetchedFeed};
use crate::indexer::{
    DEFAULT_LIMIT, IndexerAdapter, build_page_query, build_query, indexer_error, redact_query,
};
use async_trait::async_trait;
use std::sync::Arc;
use url::Url;

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

/// RD-180-20: the explicit search term travels as `q`; the title filter stays local, as
/// RD-106-10 decided -- the two are separate fields and never derived from each other.
#[tokio::test]
async fn the_explicit_search_term_is_sent_and_the_title_filter_still_is_not() {
    let fetcher = Arc::new(RecordingFetcher(std::sync::Mutex::new(None)));
    let mut subscription = indexer_subscription();
    subscription.filters.title_contains = vec!["1080p".to_owned()];
    subscription.indexer_search = rd_core::IndexerSearch {
        query: Some("some show !cam".to_owned()),
        max_age_days: Some(3),
        hide_passworded: true,
        pretime: None,
    };
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
    let queries: Vec<String> = url
        .query_pairs()
        .filter(|(key, _)| key == "q")
        .map(|(_, value)| value.into_owned())
        .collect();
    assert_eq!(queries, ["some show !cam"], "{url}");
    assert!(
        url.query_pairs().any(|(k, v)| k == "maxage" && v == "3"),
        "{url}"
    );
    assert!(
        url.query_pairs().any(|(k, v)| k == "pw" && v == "2"),
        "{url}"
    );
    assert!(url.query_pairs().all(|(key, _)| key != "pred"), "{url}");
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
