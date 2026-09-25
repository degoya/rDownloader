//! Watching a release or series page (RD-110-21).
//!
//! The address of such a subscription is a *listing*: a series page, a category, a tag, a
//! search. What a rule reads is the *release page* one entry down — the shipped rules refuse
//! a listing with `site_rules.structure`, deliberately, because a listing is not a package.
//! So this adapter does exactly one thing: it reads the listing and hands back the release
//! pages on it, each with the name the listing printed for it.
//!
//! **Everything after that is machinery that already exists.** The item's address goes into
//! the LinkGrabber through the ordinary intake, where the crawler selection asks the rules
//! (RD-110-06), the rule produces the hoster links, and RD-110-18's grouping makes the
//! mirrors one row instead of forty. Nothing about mirrors is decided or repeated here.
//!
//! **Which links are releases is the rules' answer, not a guess.** The adapter asks the
//! catalogue in force whether an address is claimed, through [`ClaimedAddresses`]. A listing
//! links to its own navigation, to the group's site and to a dozen unrelated pages; without
//! that question every one of them would become an item.
//!
//! **A listing is fetched conditionally**, with the same [`crate::FeedFetcher`] the feed
//! adapter uses: an unchanged page then costs a `304` rather than a download, which is half
//! of what makes polling somebody's board acceptable at all. The other half is the interval
//! floor in `rd_core::SubscriptionKind::min_interval_seconds` and the per-host spacing
//! RD-101-18 already puts in front of every poll.

use std::sync::Arc;

use async_trait::async_trait;
use url::Url;

use rd_core::{Subscription, SubscriptionKind};

use crate::{
    adapter::{DiscoveredItem, PollOutcome, SourceAdapter},
    feed_adapter::FeedFetcher,
    release,
};

/// Most links one listing page may contribute, whatever it links to.
///
/// A listing shows a page of releases; a document offering more than this is a sitemap or a
/// broken template, and taking all of it would fill somebody's review list in one poll.
pub const MAX_LISTING_LINKS: usize = 200;

/// Longest anchor text taken as a release name.
const MAX_TITLE: usize = 300;

/// Whether the rules in force read an address.
///
/// A trait rather than the catalogue itself, for the reason every other port in this crate is
/// one: the catalogue lives behind `rd-db` and `rd-plugin-ext`, and the order of a poll is
/// worth testing without either.
pub trait ClaimedAddresses: Send + Sync {
    /// True when some rule that is switched on claims this address.
    fn claims(&self, url: &Url) -> bool;
}

/// Polls a release or series page.
pub struct RuleAdapter {
    fetcher: Arc<dyn FeedFetcher>,
    rules: Arc<dyn ClaimedAddresses>,
}

impl RuleAdapter {
    #[must_use]
    pub fn new(fetcher: Arc<dyn FeedFetcher>, rules: Arc<dyn ClaimedAddresses>) -> Self {
        Self { fetcher, rules }
    }
}

#[async_trait]
impl SourceAdapter for RuleAdapter {
    fn kind(&self) -> SubscriptionKind {
        SubscriptionKind::SiteRule
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
            return Ok(PollOutcome {
                items: Vec::new(),
                etag: fetched.etag,
                last_modified: fetched.last_modified,
                not_modified: true,
            });
        };
        let base = fetched.final_url.as_ref().unwrap_or(&subscription.url);
        let items = self.items_of(&body, base, subscription.every_release);
        Ok(PollOutcome {
            items,
            etag: fetched.etag,
            last_modified: fetched.last_modified,
            not_modified: false,
        })
    }
}

impl RuleAdapter {
    /// Turns a listing document into the release pages on it.
    ///
    /// One entry is usually linked twice, once from its thumbnail and once from its heading,
    /// and only one of the two carries the release name. So the addresses are collapsed
    /// first and the best text among them wins; taking whichever came first would name half
    /// the items after their address.
    fn items_of(&self, body: &str, base: &Url, every_release: bool) -> Vec<DiscoveredItem> {
        let mut order: Vec<Url> = Vec::new();
        let mut texts: Vec<String> = Vec::new();
        for (href, text) in crate::rule_listing::anchors(body) {
            let Ok(url) = base.join(&href) else { continue };
            if !matches!(url.scheme(), "http" | "https") || !self.rules.claims(&url) {
                continue;
            }
            match order.iter().position(|seen| *seen == url) {
                Some(index) => {
                    if texts[index].is_empty() {
                        texts[index] = text;
                    }
                }
                None => {
                    if order.len() >= MAX_LISTING_LINKS {
                        break;
                    }
                    order.push(url);
                    texts.push(text);
                }
            }
        }
        order
            .into_iter()
            .zip(texts)
            .map(|(url, text)| {
                let title = title_of(&text, &url);
                let release = release::parse(&title);
                let mut item = DiscoveredItem::new(title, url);
                item.height = release.height();
                item.language = release.language.clone();
                // The counter-choice: with every version wanted the name decides nothing and
                // the address is the identity, exactly as for every other kind.
                item.release_key = (!every_release).then(|| release.key()).flatten();
                item
            })
            .collect()
    }
}

/// The name a listing entry gets: its anchor text, or the address's last segment when the
/// anchor carries none. A release page's slug is its release name with dashes, which is a
/// far better name than the bare address and parses as one.
fn title_of(text: &str, url: &Url) -> String {
    let text = text.trim();
    if !text.is_empty() {
        return text.chars().take(MAX_TITLE).collect();
    }
    url.path_segments()
        .and_then(|mut segments| {
            segments
                .rfind(|segment| !segment.is_empty())
                .map(|segment| segment.replace(['-', '_'], " "))
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| url.to_string())
}

#[cfg(test)]
#[path = "rule_adapter_tests.rs"]
mod rule_adapter_tests;
