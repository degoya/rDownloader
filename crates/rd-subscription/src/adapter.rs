//! The contract between the poller and a kind of source (RD-080-07).
//!
//! One trait, so the scheduler, the archive, the filters and the backlog protection are
//! written once and every source kind inherits them. RSS/Atom (RD-080-10) and
//! Newznab/Torznab (RD-080-11) are additional implementations of this trait and nothing
//! more; that is the whole reason it exists.
//!
//! An adapter's job is deliberately small: turn an address into a list of items. It does not
//! decide what is new, what is wanted, or what happens next — those are the same for every
//! source and belong to the caller.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use url::Url;

use rd_core::{Subscription, SubscriptionKind};

/// One item as a source described it, before identity or filtering.
#[derive(Clone, Debug)]
pub struct DiscoveredItem {
    /// The source's own id, when it has one. The strongest identity signal.
    pub source_id: Option<String>,
    pub title: String,
    pub url: Url,
    pub published_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<u32>,
    pub language: Option<String>,
    pub height: Option<u32>,
    /// Publication date exactly as the source wrote it, used only for the fallback hash so
    /// an unparseable date still contributes to identity.
    pub published_raw: Option<String>,
    /// The category the source filed it under, raw (RD-080-11). Mapped by the caller, not
    /// here: what it means depends on the subscription, not on the adapter.
    pub source_category: Option<String>,
    /// The media type the source declared for the address, when it declared one.
    ///
    /// An indexer's download address is an API call with no telling extension, so this is the
    /// only thing that says what it hands over without fetching it first.
    pub media_type: Option<String>,
    /// What the source said *about* the release, already filtered by
    /// [`crate::retain_attributes`] (RD-101-17): cover address, ids, resolution, size.
    ///
    /// A map rather than fields for the same reason [`crate::FeedItem`] keeps one — indexers
    /// disagree about which attributes they emit, and typing them would discard the rest.
    pub attributes: std::collections::BTreeMap<String, String>,
    /// What this item *is*, when the source's name said so (RD-110-21): a release page's
    /// series, season and episode, normalised by [`crate::release`].
    ///
    /// The strongest identity there is on a release page, and stronger than the address on
    /// purpose — the same episode posted again by another group has another address and is
    /// not another episode. `None` means "the name did not say", and identity then falls
    /// back to the address exactly as it always did.
    pub release_key: Option<String>,
    /// The archive password the source announced, when it announced one.
    ///
    /// Separate from `attributes` because it is a real value rather than Newznab's protection
    /// flag. It reaches the extractor and is shown only by authenticated subscription routes so
    /// a failed extraction can be diagnosed.
    pub password: Option<String>,
}

impl DiscoveredItem {
    /// A minimal item, for adapters that learn little more than an address.
    #[must_use]
    pub fn new(title: String, url: Url) -> Self {
        Self {
            media_type: None,
            source_id: None,
            title,
            url,
            published_at: None,
            duration_seconds: None,
            language: None,
            height: None,
            published_raw: None,
            source_category: None,
            attributes: std::collections::BTreeMap::new(),
            release_key: None,
            password: None,
        }
    }
}

/// What one poll produced.
#[derive(Clone, Debug, Default)]
pub struct PollOutcome {
    pub items: Vec<DiscoveredItem>,
    /// Caching validators to store and send next time (RD-080-10).
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// The source said nothing changed, so `items` is empty and means it.
    pub not_modified: bool,
}

/// A kind of pollable source.
#[async_trait]
pub trait SourceAdapter: Send + Sync {
    /// Which subscription kind this adapter serves.
    fn kind(&self) -> SubscriptionKind;

    /// Fetches the current items of `subscription`.
    ///
    /// Errors are the poller's business: it counts them, backs off, and stores the message
    /// after redaction. An adapter should fail rather than return a partial list, because a
    /// short list is indistinguishable from "the channel deleted everything".
    async fn poll(&self, subscription: &Subscription) -> anyhow::Result<PollOutcome>;
}

/// Picks the adapter for a subscription.
#[must_use]
pub fn adapter_for(
    adapters: &[std::sync::Arc<dyn SourceAdapter>],
    kind: SubscriptionKind,
) -> Option<&std::sync::Arc<dyn SourceAdapter>> {
    adapters.iter().find(|adapter| adapter.kind() == kind)
}
