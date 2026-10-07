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

use rd_core::{Subscription, SubscriptionId, SubscriptionKind};

use crate::{
    adapter::{DiscoveredItem, PollOutcome, RateLimited, SourceAdapter},
    feed::{FeedItem, parse_feed},
    feed_adapter::FeedFetcher,
};

/// Most results asked for in one query. Indexers cap this themselves, usually at 100.
pub const DEFAULT_LIMIT: u32 = 100;
/// Most result pages one poll reads once the subscription has an archive to meet (RD-1150-05).
///
/// A poll stops at the first page whose last entry the subscription already has, so a normal
/// poll costs a request or two and only one that never meets its archive reads them all. Twenty
/// pages are 2,000 entries between two polls, four times the old bound, which a busy category
/// reached on every poll. Deeper, a poll turns into a crawl: every page is a request against the
/// indexer's allowance, and a gap that wide is closed better by a narrower category or a shorter
/// interval — which is what the interface says when the bound is reached.
pub(crate) const MAX_PAGES: u32 = 20;
/// Pages the first poll of a subscription reads.
///
/// Nothing is archived yet, so there is nothing to meet: the depth stays what every poll read
/// before RD-1150-05, and the backlog policy decides what becomes of it.
pub(crate) const FIRST_POLL_PAGES: u32 = 5;
/// Most entries one indexer poll hands over: every page full, none repeated.
///
/// The poller archives this many (RD-1150-05). A smaller cap would leave the oldest entries of a
/// deep poll unarchived, and the next poll, stopping where it meets its archive, would never come
/// back for them.
pub const MAX_INDEXER_ITEMS: usize = MAX_PAGES as usize * DEFAULT_LIMIT as usize;
/// Newznab's error code for an exhausted request allowance ("Request limit reached").
const REQUEST_LIMIT_CODE: u32 = 500;
/// How long that refusal keeps the subscription quiet. The code names no time and the allowance
/// behind it is usually counted per day; an hour asks again the same day without spending a
/// refused request on every short interval.
const REQUEST_LIMIT_PAUSE_SECONDS: u64 = 3_600;

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
/// filter runs afterwards. A poll pages on until it meets what the subscription already has
/// (RD-1150-05), so every entry since the last poll passes the filter; the interface names the
/// page bound only when a poll reached it without meeting anything.
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
pub(crate) fn build_page_query(
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

/// Answers whether a subscription has archived an entry already (RD-1150-05).
///
/// A trait for the reason [`SecretResolver`] is one: this crate never links the database, and a
/// test supplies an archive without one. A poll asks once per result page, for its last entry —
/// one lookup on the archive's unique key, never one per entry.
#[async_trait]
pub trait ItemArchive: Send + Sync {
    async fn knows(&self, subscription: SubscriptionId, key: &str) -> anyhow::Result<bool>;
}

/// Polls Newznab and Torznab indexers.
pub struct IndexerAdapter {
    fetcher: Arc<dyn FeedFetcher>,
    secrets: Arc<dyn SecretResolver>,
    archive: Arc<dyn ItemArchive>,
}

impl IndexerAdapter {
    #[must_use]
    pub fn new(
        fetcher: Arc<dyn FeedFetcher>,
        secrets: Arc<dyn SecretResolver>,
        archive: Arc<dyn ItemArchive>,
    ) -> Self {
        Self {
            fetcher,
            secrets,
            archive,
        }
    }

    /// Fetches and reads one result page: how many entries the indexer listed on it, and the
    /// ones that can be downloaded. `None` is a first page the indexer answered with `304`.
    async fn page(
        &self,
        subscription: &Subscription,
        api_key: &str,
        page: u32,
    ) -> anyhow::Result<Option<(usize, Vec<DiscoveredItem>)>> {
        // The subscription's own search parameters (RD-180-20); empty for every subscription
        // that has none, which then polls the address it always polled.
        let url = crate::build_indexer_query(
            &subscription.url,
            api_key,
            &crate::IndexerQuery {
                limit: DEFAULT_LIMIT,
                offset: page.saturating_mul(DEFAULT_LIMIT),
                categories: &subscription.source_categories,
                search: &subscription.indexer_search,
                // A subscription asks the address it was given; the typed searches belong to
                // the LinkGrabber's search (RD-1100-03).
                typed: None,
            },
        )?;

        // Conditional headers are pointless for a search query — the answer changes with the
        // clock — so neither validator is sent and none is stored.
        let fetched = self
            .fetcher
            .fetch(&url, None, None)
            .await
            .map_err(|error| {
                // A rate limit travels as it is, so the poller waits it out rather than
                // counting a failure (RD-1150-05); it quotes no address.
                if error.is::<RateLimited>() {
                    return error;
                }
                // The address carries the key, so an error that quoted it would defeat the
                // point of storing it encrypted. Masked before it becomes a message.
                anyhow::anyhow!("{} ({})", error, redact_query(&url))
            })?;
        let Some(body) = fetched.body else {
            if page == 0 {
                return Ok(None);
            }
            anyhow::bail!("indexer returned no body for result page {}", page + 1);
        };
        // Errors come back as a document, not a status: `<error code="100"
        // description="Incorrect user credentials"/>`. Reported before parsing, or a wrong key
        // looks like an indexer with nothing new.
        if let Some(refusal) = crate::indexer_refusal(&body) {
            // An exhausted allowance is waited out like a `429` (RD-1150-05). On page three it
            // ends the poll as it would on page one: nothing read so far is kept, so the poll
            // after the pause starts from the archive again and nothing falls between them.
            if refusal.number() == Some(REQUEST_LIMIT_CODE) {
                return Err(RateLimited::after(
                    chrono::Utc::now(),
                    Some(REQUEST_LIMIT_PAUSE_SECONDS),
                )
                .into());
            }
            anyhow::bail!("indexer refused the query: {}", refusal.message());
        }

        let base = fetched.final_url.as_ref().unwrap_or(&subscription.url);
        let feed = parse_feed(&body, base)?;
        let announced = feed.items.len();
        let usable: Vec<DiscoveredItem> = feed.items.into_iter().filter_map(discovered).collect();
        // An entry with neither an enclosure nor a link cannot be downloaded, so it is dropped
        // here rather than stored. Saying so keeps "the indexer returned nothing" apart from
        // "the indexer returned entries this build cannot use".
        if usable.len() < announced {
            tracing::warn!(
                announced,
                usable = usable.len(),
                page = page + 1,
                "indexer entries without a download address were dropped"
            );
        }
        Ok(Some((announced, usable)))
    }
}

#[async_trait]
impl SourceAdapter for IndexerAdapter {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::Indexer
    }

    /// Reads result pages, newest first, until the poll meets what the subscription already
    /// has (RD-1150-05).
    ///
    /// Four ways a poll ends: a short page is the end of what the indexer lists; a page whose
    /// last entry is archived already means everything after it was there at the last poll, so
    /// the gap since then is closed; a full page that added nothing is an indexer repeating
    /// itself; and the page bound. Only the bound leaves entries unread that may be new, and a
    /// poll that reaches it says so in the log — the interface tells it from the run's counts.
    async fn poll(&self, subscription: &Subscription) -> anyhow::Result<PollOutcome> {
        let Some(reference) = subscription.secret_ref.as_deref() else {
            anyhow::bail!("indexer subscription has no API key");
        };
        let api_key = self.secrets.resolve(reference).await?;
        // The first poll has no archive to meet and keeps the depth every poll once had.
        let bound = if subscription.primed {
            MAX_PAGES
        } else {
            FIRST_POLL_PAGES
        };
        let mut items = Vec::new();
        let mut keys = HashSet::new();
        for page in 0..bound {
            let Some((announced, found)) = self.page(subscription, &api_key, page).await? else {
                return Ok(PollOutcome {
                    not_modified: true,
                    ..PollOutcome::default()
                });
            };
            let usable = found.len();
            // The *last* entry decides, not any: an indexer that lists an old release again
            // near the top must not end the poll before the new ones below it.
            let last = found.last().map(crate::key_of);
            let before = items.len();
            for item in found {
                if keys.insert(crate::key_of(&item)) {
                    items.push(item);
                }
            }
            if announced < DEFAULT_LIMIT as usize {
                return Ok(outcome(items));
            }
            if subscription.primed
                && let Some(last) = &last
                && self.archive.knows(subscription.id, last).await?
            {
                return Ok(outcome(items));
            }
            // An indexer that ignores `offset` answers every page with the first; asking on
            // would spend the whole bound on copies.
            if usable > 0 && items.len() == before {
                tracing::warn!(
                    subscription = %subscription.name,
                    page = page + 1,
                    "indexer repeated a result page; paging stopped"
                );
                return Ok(outcome(items));
            }
        }
        if subscription.primed {
            tracing::warn!(
                subscription = %subscription.name,
                pages = bound,
                found = items.len(),
                "indexer poll reached its page bound without meeting an archived entry; \
                 older new entries may be missing"
            );
        }
        Ok(outcome(items))
    }
}

/// A finished poll's items; an indexer sends no validators and asks for no pause.
fn outcome(items: Vec<DiscoveredItem>) -> PollOutcome {
    PollOutcome {
        items,
        ..PollOutcome::default()
    }
}

/// One feed entry as a discovered item; `None` for one without a download address.
fn discovered(item: FeedItem) -> Option<DiscoveredItem> {
    let url = item.download_url()?.clone();
    // `extended=1` is sent for exactly this: the attribute block travels with the search
    // answer, so what the indexer knows about a release costs no second request. Filtered
    // before it is kept — see `attributes::retain`.
    let kept = crate::retain_attributes(&item.attributes, item.size_bytes);
    Some(DiscoveredItem {
        source_id: item.id,
        title: item.title,
        url,
        published_at: item.published_at,
        duration_seconds: None,
        language: item.language,
        height: None,
        published_raw: item.published_raw,
        // Newznab reports it as an attribute; the subscription's map turns it into one of our
        // categories.
        source_category: item.categories.into_iter().next(),
        media_type: item.enclosure_type,
        attributes: kept.attributes,
        // An indexer names its own id for every release, which is a stronger signal than any
        // name; release recognition is RD-110-21's alone.
        release_key: None,
        password: kept.password,
        refused: None,
    })
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
