//! The body that creates or replaces a subscription.

use super::*;

/// Create or replace one subscription.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscriptionRequest {
    pub name: String,
    #[schema(format = "uri")]
    pub url: String,
    pub kind: SubscriptionKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: SubscriptionMode,
    #[serde(default)]
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub priority: DownloadPriority,
    #[serde(default = "default_interval")]
    pub interval_seconds: u32,
    #[serde(default)]
    pub filters: SubscriptionFilters,
    #[serde(default)]
    pub backlog: BacklogPolicy,
    /// Indexer categories routed to categories of ours (RD-080-11).
    #[serde(default)]
    pub category_map: Vec<rd_core::CategoryMapping>,
    /// Indexer categories to ask for. Empty asks for everything, as before.
    #[serde(default)]
    pub source_categories: Vec<String>,
    /// Keep every release of an episode rather than only the first (RD-110-21). Only a
    /// watched release page reads it; for every other kind identity is the address anyway.
    #[serde(default)]
    pub every_release: bool,
    /// How the LinkGrabber draws the pending hits: `list` (the default) or `cards`
    /// (RD-120-37).
    #[serde(default)]
    pub view: rd_core::SubscriptionView,
    /// Whether the card slider turns its pages on its own; off unless asked for, and ignored
    /// by the list (RD-120-37).
    #[serde(default)]
    pub autoplay: bool,
    /// The shape of a card's image area in the card view: `1:1`, `3:2`, `16:9`, `4:3` or
    /// `2:1` (the default) (RD-120-42). Anything else is refused with
    /// `subscription.card_ratio_unknown` rather than drawn as the default.
    #[serde(default = "default_card_ratio")]
    #[schema(value_type = rd_core::SubscriptionCardRatio)]
    pub card_ratio: String,
    /// A cron expression that replaces the interval: five fields in the service's local time,
    /// e.g. `0 6 * * *` for six every morning (RD-130-19). Only a `script` subscription takes
    /// one; empty or absent keeps the interval.
    #[serde(default)]
    pub schedule: Option<String>,
    /// The arguments a `script` subscription hands its script, one entry per argument, each
    /// reaching the script whole as one argv entry -- no shell splits or expands them
    /// (RD-150-08). At most 32, each at most 1024 characters, without NUL or line breaks;
    /// every other kind takes none. Stored and returned in plain text: never a secret.
    #[serde(default)]
    pub script_arguments: Vec<String>,
    /// The search term and parameters an indexer subscription sends (RD-180-20): `query` as
    /// `q` (empty or at least three characters, `!word` exclusions passed on), `max_age_days` as
    /// `maxage`, `hide_passworded` as `pw=2`, `pretime` (0-2) as `pred`. Each only when set and
    /// not already in the address. Only an indexer subscription takes them.
    #[serde(default)]
    pub indexer_search: rd_core::IndexerSearch,
    /// Which release files a `git_release` subscription downloads (RD-190-13): `forge`
    /// (`github`|`gitlab`, needed for a host other than github.com and gitlab.com),
    /// `asset_patterns` (`*`/`?` wildcards, case-insensitive, at most 32 of at most 200
    /// characters), `platforms` (`linux`|`windows`|`macos`), `architectures`
    /// (`x86_64`|`aarch64`|`x86`|`arm`), `prereleases` and `source_archives`. Drafts are never
    /// downloaded. Only a git-release subscription takes them; its read-only token is `api_key`.
    #[serde(default)]
    pub git_release: rd_core::GitReleaseOptions,
    /// A defined indexer to take over (RD-180-20): its address when `url` is empty (else `url`
    /// must be on the same server), its categories when `source_categories` is empty, and a copy
    /// of its API key when `api_key` is absent. Copied when saved, not linked.
    #[serde(default)]
    pub indexer_id: Option<rd_core::IndexerId>,
    /// Indexer API key (RD-080-11), or a git-release subscription's read-only token
    /// (RD-190-13); write-only, and stored in the vault. Omitting it on an edit keeps the
    /// existing key rather than clearing it.
    #[serde(default)]
    #[schema(write_only)]
    pub api_key: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Read as a string rather than as the enum, so an unknown ratio reaches
/// [`subscription_input`] and is refused there with a stable code, over REST and MCP alike.
fn default_card_ratio() -> String {
    rd_core::SubscriptionCardRatio::default()
        .as_str()
        .to_owned()
}

const fn default_interval() -> u32 {
    rd_core::DEFAULT_POLL_INTERVAL_SECONDS
}
