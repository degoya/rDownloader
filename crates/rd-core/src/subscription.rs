//! Subscriptions: channels, playlists, galleries, feeds and indexers that are polled on
//! their own schedule and whose new items enter the LinkGrabber (RD-080-07).
//!
//! Three ideas carry the whole feature and are worth stating before the types:
//!
//! * **Item identity is the archive.** Every item gets a canonical key that is stable across
//!   polls, and `(subscription, key)` is unique in the database. "Downloaded exactly once
//!   despite a restart and a reordered feed" is then true by construction rather than by
//!   remembering to check.
//! * **A first poll is not a backlog import.** Turning on a subscription to a channel with
//!   900 videos must not queue 900 videos. What happens on activation is an explicit choice.
//! * **A filter decision is explainable.** An item that was skipped records *which* rule
//!   skipped it, because "nothing appeared" is otherwise indistinguishable from a broken
//!   filter, a broken adapter and an empty channel.

mod archive;
mod filters;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{CategoryId, DownloadPriority, SubscriptionId};

pub use archive::{
    SubscriptionBulkStateResponse, SubscriptionHistoryClearResponse, SubscriptionItem,
    SubscriptionItemCounts, SubscriptionItemPage, SubscriptionItemState, SubscriptionReviewCount,
    SubscriptionReviewSummary, SubscriptionRun,
};
pub use filters::{FilterReason, SubscriptionFilters};

/// Shortest poll interval a subscription may be given, in seconds.
///
/// Polling a third party every few seconds is rude and gets the address blocked; nothing
/// this feature does is urgent enough to justify it.
pub const MIN_POLL_INTERVAL_SECONDS: u32 = 300;
/// Shortest poll interval for a subscription on a release page (RD-110-21).
///
/// Half an hour. A release page appears once and stays; asking every five minutes finds the
/// same page 359 times out of 360 and reads like a crawler to the server that serves it.
pub const SITE_RULE_MIN_POLL_INTERVAL_SECONDS: u32 = 1_800;
/// Longest poll interval, so a subscription cannot be configured into never running.
pub const MAX_POLL_INTERVAL_SECONDS: u32 = 7 * 24 * 60 * 60;
/// Default interval for a new subscription.
pub const DEFAULT_POLL_INTERVAL_SECONDS: u32 = 3_600;
/// Most items one poll may take from a source, however many it offers.
pub const MAX_ITEMS_PER_POLL: usize = 500;
/// Longest accepted canonical item key.
pub const MAX_ITEM_KEY: usize = 512;
/// Most filter patterns of one kind.
pub const MAX_FILTER_PATTERNS: usize = 32;
/// Most category mappings one subscription may carry (RD-080-11).
pub const MAX_CATEGORY_MAPPINGS: usize = 200;

/// Where a subscription's items come from.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionKind {
    /// A yt-dlp channel, user or playlist page.
    #[default]
    Media,
    /// A gallery-dl profile.
    Gallery,
    /// An RSS/Atom feed or podcast (RD-080-10).
    Feed,
    /// A Newznab/Torznab indexer query (RD-080-11).
    Indexer,
    /// A release or series page read through a site rule (RD-110-21).
    SiteRule,
    /// A script from the scripts directory whose output lines are links (RD-130-19).
    ///
    /// It starts code on the machine, so only the administrator may create or change one,
    /// and no road that moves configuration in bulk -- MCP, import, backup -- carries it.
    Script,
}

impl SubscriptionKind {
    /// The shortest interval this kind may be polled at.
    ///
    /// A board is not an API. An indexer and a feed publish a document made to be fetched
    /// often; a release page is a page somebody's server renders, and the rule that reads it
    /// fetches more than one of them. Five minutes is polite for the first two and rude for
    /// the last, so the floor is per kind rather than one number for everything.
    #[must_use]
    pub const fn min_interval_seconds(self) -> u32 {
        match self {
            Self::SiteRule => SITE_RULE_MIN_POLL_INTERVAL_SECONDS,
            _ => MIN_POLL_INTERVAL_SECONDS,
        }
    }

    /// Whether the first poll protects against a backlog (RD-130-19).
    ///
    /// A channel or a feed has a history, and its first poll must not import a decade of it.
    /// A script has none: it prints what it was written to find *now*, and somebody who set
    /// one up wants its first run taken, not recorded as the past.
    #[must_use]
    pub const fn has_backlog(self) -> bool {
        !matches!(self, Self::Script)
    }
}

/// The address scheme a script subscription stores its script name under (RD-130-19).
///
/// `script:<name>` in the `url` column, which is `NOT NULL` and a URL everywhere it is read;
/// a scheme of its own keeps it from ever being mistaken for something to fetch.
pub const SCRIPT_URL_SCHEME: &str = "script";

/// What happens to an item that passes the filters.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionMode {
    /// Collected for a person to look at. The default, deliberately: a subscription that
    /// starts queueing on its own is hard to undo.
    #[default]
    Review,
    /// Handed straight to intake, where routing rules and categories still apply.
    AutoQueue,
}

/// How the LinkGrabber draws a subscription's hits still waiting for a decision (RD-120-37).
///
/// A presentation choice made per subscription rather than once for the application: a music
/// search reads best as covers to flip through, a series search as the dense list. The list is
/// the default because it is what every subscription showed before the choice existed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionView {
    /// One row per hit, details behind a chevron.
    #[default]
    List,
    /// A slider of equally sized cards, details in one panel below it.
    Cards,
}

impl SubscriptionView {
    /// The stored and serialised spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Cards => "cards",
        }
    }

    /// Reads the stored spelling; anything unknown is the list, which is what the column
    /// defaults to and what a reader who knows no other view can always draw.
    #[must_use]
    pub fn from_stored(value: &str) -> Self {
        match value {
            "cards" => Self::Cards,
            _ => Self::List,
        }
    }
}

/// The shape of a card's image area in the card view (RD-120-42).
///
/// Chosen per subscription because the pictures differ per source: a music search brings
/// square covers, a series search wide banners. `2:1` is the default because it is closest to
/// the fixed height every card had before the choice existed. The spelling on the wire and in
/// the store is the ratio itself, so a reader never has to translate a name back into numbers.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub enum SubscriptionCardRatio {
    #[serde(rename = "1:1")]
    OneOne,
    /// Portrait, for poster art (RD-120-42, added on the owner's request after the first test).
    #[serde(rename = "2:3")]
    TwoThree,
    #[serde(rename = "3:2")]
    ThreeTwo,
    #[serde(rename = "16:9")]
    SixteenNine,
    #[serde(rename = "4:3")]
    FourThree,
    #[default]
    #[serde(rename = "2:1")]
    TwoOne,
}

impl SubscriptionCardRatio {
    /// Every ratio, in the order the form offers them.
    pub const ALL: [Self; 6] = [
        Self::OneOne,
        Self::TwoThree,
        Self::ThreeTwo,
        Self::SixteenNine,
        Self::FourThree,
        Self::TwoOne,
    ];

    /// The stored and serialised spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneOne => "1:1",
            Self::TwoThree => "2:3",
            Self::ThreeTwo => "3:2",
            Self::SixteenNine => "16:9",
            Self::FourThree => "4:3",
            Self::TwoOne => "2:1",
        }
    }

    /// Reads a spelling somebody sent; `None` for anything that is not one of the six, so the
    /// caller can refuse it rather than draw something nobody asked for.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ratio| ratio.as_str() == value)
    }

    /// Reads the stored spelling; anything unknown is the default the column carries.
    #[must_use]
    pub fn from_stored(value: &str) -> Self {
        Self::parse(value).unwrap_or_default()
    }
}

/// What the *first* poll of a subscription does with everything already there.
///
/// This exists because the alternative is a channel with a decade of uploads arriving in
/// the queue at once, which is the single most destructive thing this feature could do.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "mode", content = "since")]
pub enum BacklogPolicy {
    /// Record everything present at activation as already seen, and act only on what
    /// appears afterwards.
    #[default]
    FromNow,
    /// Act on items published at or after this instant; older ones are marked seen.
    Since(DateTime<Utc>),
    /// Collect the whole backlog for review. Never queues it, whatever the mode says.
    ReviewAll,
}

/// One indexer category routed to a category of ours (RD-080-11).
///
/// A pair rather than a map so it round-trips through OpenAPI as a list the UI can render
/// and reorder; the lookup builds a map from it when it needs one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CategoryMapping {
    /// The indexer's own category id, e.g. `5040`.
    pub source_category: String,
    /// Where releases in it should go.
    pub category_id: CategoryId,
}

/// A polled source.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Subscription {
    pub id: SubscriptionId,
    pub name: String,
    #[schema(value_type = String, format = "uri")]
    pub url: Url,
    pub kind: SubscriptionKind,
    pub enabled: bool,
    pub mode: SubscriptionMode,
    /// Destination category of accepted items; `None` = the default category.
    pub category_id: Option<CategoryId>,
    #[serde(default)]
    pub priority: DownloadPriority,
    /// Seconds between polls, clamped to [`MIN_POLL_INTERVAL_SECONDS`]..=
    /// [`MAX_POLL_INTERVAL_SECONDS`].
    pub interval_seconds: u32,
    #[serde(default)]
    pub filters: SubscriptionFilters,
    #[serde(default)]
    pub backlog: BacklogPolicy,
    /// Indexer categories routed to categories of ours (RD-080-11). Empty means every item
    /// lands in `category_id`.
    #[serde(default)]
    pub category_map: Vec<CategoryMapping>,
    /// The indexer categories to ask for, sent as `cat`. Empty means "everything", which is
    /// what every subscription written before this field did.
    ///
    /// Deliberately not derived from `category_map`: the two answer different questions. One
    /// can want a category fetched without redirecting it anywhere — it lands in the default —
    /// and one can keep a mapping for a category one is not fetching at the moment. Folding
    /// them together would make each choice change the other.
    #[serde(default)]
    pub source_categories: Vec<String>,
    /// Whether the first poll has happened. Until it has, the backlog policy applies.
    #[serde(default)]
    pub primed: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    /// When the scheduler intends to poll next; also what a backoff pushes out.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Consecutive failed polls, which is what the backoff is computed from.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Last poll error, cleared by a successful poll. Redacted before it is stored.
    pub last_error: Option<String>,
    /// Feed caching (RD-080-10); ignored by the other kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// Vault reference of an indexer API key (RD-080-11). Never serialised.
    #[serde(skip_serializing)]
    #[schema(ignore)]
    pub secret_ref: Option<String>,
    /// Whether a key is stored, which is all a client is told about it.
    #[serde(default)]
    pub has_secret: bool,
    /// Keep every release of an episode rather than only the first (RD-110-21).
    ///
    /// The default is `false`, which is the whole point of a release-page subscription: the
    /// same episode is posted again in another group's release, another quality and another
    /// week, and without this being off a subscription enqueues it every time. `true` is the
    /// explicit counter-choice for somebody who collects versions.
    #[serde(default)]
    pub every_release: bool,
    /// How the LinkGrabber draws this subscription's pending hits (RD-120-37).
    #[serde(default)]
    pub view: SubscriptionView,
    /// Whether the card slider turns its pages on its own (RD-120-37). Off by default, and
    /// meaningless for the list; the interface pauses it whenever the person is using it.
    #[serde(default)]
    pub autoplay: bool,
    /// The shape of a card's image area in the card view (RD-120-42); `2:1` by default.
    #[serde(default)]
    pub card_ratio: SubscriptionCardRatio,
    /// A cron expression (five fields, the service's local time) that replaces the interval
    /// when set (RD-130-19). Only a script subscription carries one.
    #[serde(default)]
    pub schedule: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Subscription {
    /// The category an item of `source_category` belongs in.
    ///
    /// Falls back to the subscription's own category, so an unmapped or absent source
    /// category behaves exactly as it did before mapping existed.
    #[must_use]
    pub fn category_for(&self, source_category: Option<&str>) -> Option<CategoryId> {
        source_category
            .and_then(|source| {
                self.category_map
                    .iter()
                    .find(|mapping| mapping.source_category == source)
                    .map(|mapping| mapping.category_id)
            })
            .or(self.category_id)
    }

    /// The script a script subscription runs: the name under the `script:` address.
    ///
    /// `None` for every other kind, and for a script address that carries nothing.
    #[must_use]
    pub fn script_name(&self) -> Option<&str> {
        if self.kind != SubscriptionKind::Script || self.url.scheme() != SCRIPT_URL_SCHEME {
            return None;
        }
        Some(self.url.path()).filter(|name| !name.is_empty())
    }

    /// Whether this subscription should be polled at `now`.
    #[must_use]
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.enabled && self.next_run_at.is_none_or(|next| next <= now)
    }

    /// The interval, clamped to the range this subscription's kind permits.
    ///
    /// The floor is the kind's, not the global one (RD-110-21): a subscription stored with
    /// five minutes — by an older build, by the API, or by a person typing it — polls a
    /// release page every half hour anyway. Clamping here rather than refusing the value
    /// means the cap holds for rows that already exist and for every road into the poller.
    #[must_use]
    pub fn effective_interval(&self) -> u32 {
        self.interval_seconds
            .clamp(self.kind.min_interval_seconds(), MAX_POLL_INTERVAL_SECONDS)
    }
}

/// Service-wide subscription settings (part of the `service.settings` blob, keys prefixed
/// `subscription_`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SubscriptionSettings {
    /// Whether the poller runs at all.
    pub subscription_enabled: bool,
    /// Subscriptions polled concurrently. A failing source must not hold up the others,
    /// but neither should a hundred of them start at once.
    pub subscription_max_parallel: u32,
    /// Seconds the scheduler sleeps between looking for due subscriptions.
    pub subscription_tick_seconds: u32,
    /// Timeout for one poll.
    pub subscription_poll_timeout_seconds: u32,
}

impl Default for SubscriptionSettings {
    fn default() -> Self {
        Self {
            subscription_enabled: true,
            subscription_max_parallel: 2,
            subscription_tick_seconds: 60,
            subscription_poll_timeout_seconds: 120,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS, MIN_POLL_INTERVAL_SECONDS,
        SITE_RULE_MIN_POLL_INTERVAL_SECONDS, Subscription, SubscriptionKind, SubscriptionMode,
    };
    use chrono::{Duration, Utc};

    fn subscription(interval: u32) -> Subscription {
        Subscription {
            id: crate::SubscriptionId::new(),
            name: "Channel".to_owned(),
            url: "https://example.test/c/x".parse().expect("url"),
            kind: SubscriptionKind::Media,
            enabled: true,
            mode: SubscriptionMode::Review,
            category_id: None,
            priority: crate::DownloadPriority::default(),
            interval_seconds: interval,
            filters: super::SubscriptionFilters::default(),
            backlog: super::BacklogPolicy::default(),
            category_map: Vec::new(),
            source_categories: Vec::new(),
            primed: false,
            last_run_at: None,
            next_run_at: None,
            consecutive_failures: 0,
            last_error: None,
            etag: None,
            last_modified: None,
            secret_ref: None,
            has_secret: false,
            every_release: false,
            view: super::SubscriptionView::List,
            autoplay: false,
            card_ratio: super::SubscriptionCardRatio::TwoOne,
            schedule: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn an_interval_is_clamped_into_the_permitted_range() {
        // Polling somebody else's server every second gets the address blocked, and nothing
        // here is urgent enough to be worth that.
        assert_eq!(
            subscription(1).effective_interval(),
            MIN_POLL_INTERVAL_SECONDS
        );
        assert_eq!(
            subscription(u32::MAX).effective_interval(),
            MAX_POLL_INTERVAL_SECONDS
        );
        assert_eq!(
            subscription(DEFAULT_POLL_INTERVAL_SECONDS).effective_interval(),
            DEFAULT_POLL_INTERVAL_SECONDS
        );
    }

    #[test]
    fn a_release_page_is_never_polled_faster_than_its_own_floor() {
        // RD-110-21. The floor is the kind's, so a row stored with the global minimum --
        // by an older build or by a person typing it -- is still polled politely.
        let mut watched = subscription(MIN_POLL_INTERVAL_SECONDS);
        watched.kind = SubscriptionKind::SiteRule;
        assert_eq!(
            watched.effective_interval(),
            SITE_RULE_MIN_POLL_INTERVAL_SECONDS
        );
        watched.interval_seconds = 1;
        assert_eq!(
            watched.effective_interval(),
            SITE_RULE_MIN_POLL_INTERVAL_SECONDS
        );
        // A longer interval than the floor is the person's business and stays.
        watched.interval_seconds = 6 * 60 * 60;
        assert_eq!(watched.effective_interval(), 6 * 60 * 60);
        // Every other kind keeps the global floor it always had.
        for kind in [
            SubscriptionKind::Media,
            SubscriptionKind::Gallery,
            SubscriptionKind::Feed,
            SubscriptionKind::Indexer,
        ] {
            assert_eq!(kind.min_interval_seconds(), MIN_POLL_INTERVAL_SECONDS);
        }
        const { assert!(SITE_RULE_MIN_POLL_INTERVAL_SECONDS > MIN_POLL_INTERVAL_SECONDS) };
    }

    #[test]
    fn a_subscription_with_no_next_run_is_due_immediately() {
        assert!(subscription(600).is_due(Utc::now()));
    }

    #[test]
    fn a_disabled_subscription_is_never_due() {
        let mut disabled = subscription(600);
        disabled.enabled = false;
        assert!(!disabled.is_due(Utc::now()));
    }

    #[test]
    fn a_future_next_run_is_respected() {
        let now = Utc::now();
        let mut later = subscription(600);
        later.next_run_at = Some(now + Duration::minutes(5));
        assert!(!later.is_due(now));
        assert!(later.is_due(now + Duration::minutes(6)));
    }

    #[test]
    fn an_unmapped_category_falls_back_to_the_subscription_s_own() {
        // The behaviour that existed before mapping did, and what an indexer category
        // nobody has mapped yet has to keep doing.
        let default_category = crate::CategoryId::new();
        let mut subscription = subscription(600);
        subscription.category_id = Some(default_category);
        assert_eq!(subscription.category_for(None), Some(default_category));
        assert_eq!(
            subscription.category_for(Some("9999")),
            Some(default_category)
        );
    }

    #[test]
    fn a_mapped_category_wins_over_the_default() {
        let default_category = crate::CategoryId::new();
        let tv = crate::CategoryId::new();
        let mut subscription = subscription(600);
        subscription.category_id = Some(default_category);
        subscription.category_map = vec![super::CategoryMapping {
            source_category: "5040".to_owned(),
            category_id: tv,
        }];
        assert_eq!(subscription.category_for(Some("5040")), Some(tv));
        // Only the mapped one: a near miss must not borrow another category's mapping.
        assert_eq!(
            subscription.category_for(Some("5030")),
            Some(default_category)
        );
    }

    #[test]
    fn a_subscription_without_a_category_still_maps() {
        let tv = crate::CategoryId::new();
        let mut subscription = subscription(600);
        subscription.category_id = None;
        subscription.category_map = vec![super::CategoryMapping {
            source_category: "5040".to_owned(),
            category_id: tv,
        }];
        assert_eq!(subscription.category_for(Some("5040")), Some(tv));
        // Unmapped and no default: the routing rules decide, as they always did.
        assert_eq!(subscription.category_for(Some("2040")), None);
    }

    #[test]
    fn a_script_has_no_backlog_and_names_its_script_under_its_own_scheme() {
        // RD-130-19. Every other kind keeps the backlog protection it always had.
        assert!(!SubscriptionKind::Script.has_backlog());
        for kind in [
            SubscriptionKind::Media,
            SubscriptionKind::Gallery,
            SubscriptionKind::Feed,
            SubscriptionKind::Indexer,
            SubscriptionKind::SiteRule,
        ] {
            assert!(kind.has_backlog(), "{kind:?}");
        }
        let mut script = subscription(DEFAULT_POLL_INTERVAL_SECONDS);
        script.kind = SubscriptionKind::Script;
        script.url = "script:daily-links.sh".parse().expect("url");
        assert_eq!(script.script_name(), Some("daily-links.sh"));
        // The same address on another kind names nothing, and so does an http address.
        script.kind = SubscriptionKind::Feed;
        assert_eq!(script.script_name(), None);
        script.kind = SubscriptionKind::Script;
        script.url = "https://example.test/links.sh".parse().expect("url");
        assert_eq!(script.script_name(), None);
        assert_eq!(
            serde_json::to_value(SubscriptionKind::Script).expect("serialise"),
            serde_json::json!("script")
        );
    }

    #[test]
    fn review_is_the_default_mode() {
        // A subscription that starts queueing on its own is hard to undo.
        assert_eq!(SubscriptionMode::default(), SubscriptionMode::Review);
    }

    #[test]
    fn the_view_defaults_to_the_list_and_round_trips_its_spelling() {
        use super::SubscriptionView;
        assert_eq!(SubscriptionView::default(), SubscriptionView::List);
        for view in [SubscriptionView::List, SubscriptionView::Cards] {
            assert_eq!(SubscriptionView::from_stored(view.as_str()), view);
            let json = serde_json::to_value(view).expect("serialise");
            assert_eq!(json, serde_json::Value::String(view.as_str().to_owned()));
        }
        assert_eq!(
            SubscriptionView::from_stored("carousel"),
            SubscriptionView::List
        );
    }

    #[test]
    fn a_subscription_written_before_the_view_existed_reads_as_a_list_without_autoplay() {
        let mut json =
            serde_json::to_value(subscription(DEFAULT_POLL_INTERVAL_SECONDS)).expect("serialise");
        let object = json.as_object_mut().expect("object");
        object.remove("view");
        object.remove("autoplay");
        let read: Subscription = serde_json::from_value(json).expect("deserialise");
        assert_eq!(read.view, super::SubscriptionView::List);
        assert!(!read.autoplay);
    }

    #[test]
    fn the_card_ratio_defaults_to_two_to_one_and_refuses_what_it_does_not_know() {
        use super::SubscriptionCardRatio;
        assert_eq!(
            SubscriptionCardRatio::default(),
            SubscriptionCardRatio::TwoOne
        );
        for ratio in SubscriptionCardRatio::ALL {
            assert_eq!(SubscriptionCardRatio::parse(ratio.as_str()), Some(ratio));
            assert_eq!(SubscriptionCardRatio::from_stored(ratio.as_str()), ratio);
            let json = serde_json::to_value(ratio).expect("serialise");
            assert_eq!(json, serde_json::Value::String(ratio.as_str().to_owned()));
        }
        // Portrait, asked for after the first live test of the card view.
        assert_eq!(
            SubscriptionCardRatio::parse("2:3"),
            Some(SubscriptionCardRatio::TwoThree)
        );
        for unknown in ["21:9", "2/1", "", " 2:1", "square"] {
            assert_eq!(SubscriptionCardRatio::parse(unknown), None, "{unknown:?}");
        }
        assert!(
            serde_json::from_value::<SubscriptionCardRatio>(serde_json::json!("21:9")).is_err()
        );
        assert_eq!(
            SubscriptionCardRatio::from_stored("21:9"),
            SubscriptionCardRatio::TwoOne
        );
    }

    #[test]
    fn a_subscription_written_before_the_card_ratio_existed_reads_as_two_to_one() {
        let mut json =
            serde_json::to_value(subscription(DEFAULT_POLL_INTERVAL_SECONDS)).expect("serialise");
        json.as_object_mut().expect("object").remove("card_ratio");
        let read: Subscription = serde_json::from_value(json).expect("deserialise");
        assert_eq!(read.card_ratio, super::SubscriptionCardRatio::TwoOne);
    }
}
