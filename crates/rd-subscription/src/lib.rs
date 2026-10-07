//! Subscriptions: polling channels, playlists, galleries, feeds, indexers, scripts and the
//! releases of GitHub and GitLab repositories (RD-190-13) on their own schedule, and turning
//! what is new into LinkGrabber intake (RD-080-07).
//!
//! The crate is the *logic* half — identity, filtering, scheduling and the source adapters.
//! The domain types live in `rd-core` because the database and the API both speak them, and
//! the persistence and the background task live where every other one does, in `rd-db` and
//! `rd-api` respectively.
//!
//! Everything here is pure or trait-bounded, which is deliberate: the interesting behaviour
//! of a poller is all about clocks, duplicates and partial failure, and none of those are
//! worth reproducing by waiting for a real feed.

#![warn(unreachable_pub)]

mod adapter;
mod attributes;
mod caps;
mod feed;
mod feed_adapter;
mod filter;
mod git_release;
mod git_release_adapter;
mod identity;
mod indexer;
mod media_adapter;
mod query;
mod release;
mod rule_adapter;
mod rule_listing;
mod schedule;
mod script_adapter;
mod search;
#[cfg(test)]
mod test_support;

pub use adapter::{DiscoveredItem, PollOutcome, RateLimited, SourceAdapter, adapter_for};
pub use attributes::{
    MAX_FIELDS as MAX_ATTRIBUTE_FIELDS, MAX_TOTAL as MAX_ATTRIBUTE_TOTAL,
    MAX_VALUE as MAX_ATTRIBUTE_VALUE, RetainedAttributes, retain as retain_attributes,
};
pub use caps::{IndexerCaps, IndexerCategory, MAX_CAPS_BYTES, MAX_CAPS_CATEGORIES, parse_caps};
pub use feed::{
    Feed, FeedItem, MAX_FEED_BYTES, MAX_FEED_ITEMS, parse_date, parse_duration, parse_feed,
};
pub use feed_adapter::{FeedAdapter, FeedFetcher, FetchedFeed};
pub use filter::{CandidateItem, Decision, PreparedFilters, evaluate};
pub use git_release::{
    ChecksumFile, Repository as GitRepository, architecture_of, checksum_file, glob_matches,
    looks_like_prerelease, parse_checksums, platform_of, selects as git_release_selects,
};
pub use git_release_adapter::{
    ApiFetcher, ApiResponse, GitReleaseAdapter, MAX_CHECKSUM_BYTES, MAX_RELEASE_LIST_BYTES,
    RELEASES_PER_POLL, rate_limit_pause,
};
pub use identity::{ItemIdentity, item_key, normalize_url};
pub use indexer::{
    DEFAULT_LIMIT, IndexerAdapter, ItemArchive, MAX_INDEXER_ITEMS, SecretResolver,
    build_caps_query, build_query, indexer_error, redact_query,
};
pub use media_adapter::{MediaAdapter, parse_upload_date};
pub use query::{IndexerQuery, IndexerSearchType, TypedSearch, build_indexer_query};
pub use release::{ReleaseName, parse as parse_release_name};
pub use rule_adapter::{ClaimedAddresses, MAX_LISTING_LINKS, RuleAdapter};
pub use schedule::{
    JITTER_PERCENT, MAX_BACKOFF_SECONDS, MAX_SCHEDULE_LEN, next_failure, next_scheduled,
    next_scheduled_failure, next_success, parse_schedule,
};
pub use script_adapter::{ScriptAdapter, ScriptRunner, links_of as script_links};
pub use search::{
    IndexerRefusal, MAX_SEARCH_LIMIT, SearchHit, SearchPage, indexer_refusal, parse_search,
};

/// Builds the [`CandidateItem`] the filters judge from a [`DiscoveredItem`].
#[must_use]
pub fn candidate_of(item: &DiscoveredItem) -> CandidateItem {
    CandidateItem {
        title: item.title.clone(),
        published_at: item.published_at,
        duration_seconds: item.duration_seconds,
        language: item.language.clone(),
        height: item.height,
    }
}

/// The canonical key for a discovered item.
#[must_use]
pub fn key_of(item: &DiscoveredItem) -> String {
    item_key(&ItemIdentity {
        source_id: item.source_id.as_deref(),
        url: Some(&item.url),
        title: Some(&item.title),
        published: item.published_raw.as_deref(),
        release: item.release_key.as_deref(),
    })
}
