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

/// Builds one page of an indexer query with no search parameters.
///
/// The subscription's spelling of [`crate::build_indexer_query`], kept so the address a
/// subscription without search parameters polls is exactly the one it always polled.
pub fn build_page_query(
    base: &Url,
    api_key: &str,
    limit: u32,
    offset: u32,
    categories: &[String],
) -> anyhow::Result<Url> {
    crate::build_indexer_query(
        base,
        api_key,
        &crate::IndexerQuery {
            limit,
            offset,
            categories,
            search: &rd_core::IndexerSearch::default(),
            typed: None,
        },
    )
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
            // The subscription's own search parameters (RD-180-20); empty for every
            // subscription that has none, which then polls the address it always polled.
            let url = crate::build_indexer_query(
                &subscription.url,
                &api_key,
                &crate::IndexerQuery {
                    limit: DEFAULT_LIMIT,
                    offset,
                    categories: &subscription.source_categories,
                    search: &subscription.indexer_search,
                    // A subscription asks the address it was given; the typed searches belong
                    // to the LinkGrabber's search (RD-1100-03).
                    typed: None,
                },
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
                    refused: None,
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
            paused_until: None,
        })
    }
}

/// Extracts a Newznab/Torznab `<error …>` description, if the document is one.
///
/// The one-line form of [`crate::indexer_refusal`], which is what a poll's stored error and a
/// log line carry.
#[must_use]
pub fn indexer_error(body: &str) -> Option<String> {
    crate::indexer_refusal(body).map(|refusal| refusal.message())
}

#[cfg(test)]
#[path = "indexer_tests.rs"]
mod tests;
