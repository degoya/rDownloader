//! Persistence of subscriptions, their items and their poll history (RD-080-07).
//!
//! The interesting method here is [`record_items`]. It relies on the UNIQUE index over
//! `(subscription_id, item_key)` rather than on a read-then-write: two polls that overlap,
//! or a poll interrupted halfway and repeated after a restart, must not produce two rows
//! for one item, and `INSERT … ON CONFLICT` against the UNIQUE index is the only version of
//! that which is true without holding a lock across the network call.
//!
//! A repeat poll refreshes an item nobody has decided on yet — its address and the media type
//! the feed declares — because those are the feed's to correct and a stale one is what an
//! archived item is stuck with otherwise. Anything already queued, skipped or dismissed is
//! left exactly as it is: re-listing an entry must never undo a decision.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{
    BacklogPolicy, CategoryId, DownloadPriority, EventEnvelope, EventKind, FilterReason,
    Subscription, SubscriptionCardRatio, SubscriptionFilters, SubscriptionId, SubscriptionItem,
    SubscriptionItemCounts, SubscriptionItemId, SubscriptionItemPage, SubscriptionItemState,
    SubscriptionKind, SubscriptionMode, SubscriptionReviewCount, SubscriptionReviewSummary,
    SubscriptionRun, SubscriptionView,
};
use sqlx::SqlitePool;
use url::Url;

#[path = "subscription_store_items.rs"]
mod items;
#[path = "subscription_store_rows.rs"]
mod rows;
#[path = "subscription_store_writes.rs"]
mod writes;

pub(crate) use items::{
    ItemPasswords, clear_history, finish_run, record_items, set_item_state, set_pending_items_state,
};
use rows::{ItemCountRow, ItemRow, ReviewCountRow, RunRow, SubscriptionRow};
pub(crate) use writes::{arm, create, delete, set_enabled, update};

/// Editable fields of a subscription; `create` assigns the id and timestamps.
#[derive(Clone, Debug)]
pub struct NewSubscription {
    pub name: String,
    pub url: Url,
    pub kind: SubscriptionKind,
    pub enabled: bool,
    pub mode: SubscriptionMode,
    pub category_id: Option<CategoryId>,
    pub priority: DownloadPriority,
    pub interval_seconds: u32,
    pub filters: SubscriptionFilters,
    pub backlog: BacklogPolicy,
    pub category_map: Vec<rd_core::CategoryMapping>,
    pub source_categories: Vec<String>,
    /// Keep every release of an episode rather than only the first (RD-110-21).
    pub every_release: bool,
    /// How the LinkGrabber draws the pending hits, and whether the cards turn on their own
    /// (RD-120-37).
    pub view: SubscriptionView,
    pub autoplay: bool,
    /// The shape of a card's image area in the card view (RD-120-42).
    pub card_ratio: SubscriptionCardRatio,
    /// A cron expression that replaces the interval (RD-130-19), already validated.
    pub schedule: Option<String>,
    /// The arguments a script subscription hands its script (RD-150-08), already validated.
    pub script_arguments: Vec<String>,
    /// The search parameters an indexer subscription sends (RD-180-20), already validated.
    pub indexer_search: rd_core::IndexerSearch,
    /// Which assets a git-release subscription downloads (RD-190-13), already validated.
    pub git_release: rd_core::GitReleaseOptions,
    pub secret_ref: Option<String>,
}

/// One item as the poller decided it, ready to be archived.
#[derive(Clone, Debug)]
pub struct NewSubscriptionItem {
    pub item_key: String,
    pub title: String,
    pub url: Url,
    pub published_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<u32>,
    pub state: SubscriptionItemState,
    pub reason: Option<FilterReason>,
    pub source_category: Option<String>,
    /// The media type the source declared for `url`, when it declared one.
    pub media_type: Option<String>,
    /// What the source said about the release, already filtered (RD-101-17).
    pub attributes: BTreeMap<String, String>,
    /// The archive password the source announced. Stored apart from `attributes` because it
    /// is a secret: in the vault, never in the row (RD-190-04), and read back only by the
    /// intake that hands it to the package and the review list that shows it.
    pub password: Option<String>,
}

/// Outcome of one poll, written when it finishes.
#[derive(Clone, Debug)]
pub struct PollResult {
    pub found: u32,
    pub accepted: u32,
    pub skipped: u32,
    pub error: Option<String>,
    pub next_run_at: DateTime<Utc>,
    pub consecutive_failures: u32,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

const COLUMNS: &str = "id, name, url, kind, enabled, mode, category_id, priority, \
     interval_seconds, filters_json, backlog_json, category_map_json, \
     source_categories_json, every_release, view, autoplay, card_ratio, schedule, \
     script_arguments_json, indexer_search_json, git_release_json, primed, last_run_at, next_run_at, consecutive_failures, last_error, etag, last_modified, \
     secret_ref, created_at, updated_at";

const ITEM_COLUMNS: &str = "id, subscription_id, item_key, title, url, published_at, \
     duration_seconds, state, reason, source_category, media_type, attributes_json, \
     discovered_at";

fn changed_event() -> EventEnvelope {
    EventEnvelope::new(
        EventKind::SubscriptionChanged,
        serde_json::json!({ "resource": "subscription" }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Subscription>> {
    sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM subscriptions ORDER BY name, created_at"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn get(pool: &SqlitePool, id: SubscriptionId) -> Result<Option<Subscription>> {
    sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM subscriptions WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

/// Subscriptions whose next run has come, oldest due first.
///
/// A row with no `next_run_at` has never run and is due immediately, which is what makes a
/// freshly created subscription poll without waiting a full interval.
pub(crate) async fn due(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<Subscription>> {
    sqlx::query_as::<_, SubscriptionRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM subscriptions \
         WHERE enabled = 1 AND (next_run_at IS NULL OR next_run_at <= ?) \
         ORDER BY next_run_at IS NOT NULL, next_run_at"
    )))
    .bind(now)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// One state-filtered archive page plus totals calculated across the complete subscription.
pub(crate) async fn item_page(
    pool: &SqlitePool,
    id: SubscriptionId,
    state: Option<SubscriptionItemState>,
    limit: i64,
    offset: i64,
) -> Result<SubscriptionItemPage> {
    let state = state.map(item_state_string);
    let rows = sqlx::query_as::<_, ItemRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {ITEM_COLUMNS} FROM subscription_items \
         WHERE subscription_id = ? AND (? IS NULL OR state = ?) \
         ORDER BY discovered_at DESC, id DESC LIMIT ? OFFSET ?"
    )))
    .bind(id.to_string())
    .bind(state)
    .bind(state)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM subscription_items \
         WHERE subscription_id = ? AND (? IS NULL OR state = ?)",
    )
    .bind(id.to_string())
    .bind(state)
    .bind(state)
    .fetch_one(pool)
    .await?;
    let counts = item_counts(pool, id).await?;
    let run_total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM subscription_runs WHERE subscription_id = ?",
    )
    .bind(id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(SubscriptionItemPage {
        items: rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>>>()?,
        total: u64::try_from(total).unwrap_or_default(),
        counts,
        run_total: u64::try_from(run_total).unwrap_or_default(),
    })
}

async fn item_counts(pool: &SqlitePool, id: SubscriptionId) -> Result<SubscriptionItemCounts> {
    let row = sqlx::query_as::<_, ItemCountRow>(
        "SELECT \
           COALESCE(SUM(CASE WHEN state = 'pending' THEN 1 ELSE 0 END), 0) AS pending, \
           COALESCE(SUM(CASE WHEN state = 'queued' THEN 1 ELSE 0 END), 0) AS queued, \
           COALESCE(SUM(CASE WHEN state = 'skipped' THEN 1 ELSE 0 END), 0) AS skipped, \
           COALESCE(SUM(CASE WHEN state = 'dismissed' THEN 1 ELSE 0 END), 0) AS dismissed \
         FROM subscription_items WHERE subscription_id = ?",
    )
    .bind(id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(row.into())
}

/// Pending counts for indexer subscriptions, including zeroes so the UI can render every one.
pub(crate) async fn review_summary(pool: &SqlitePool) -> Result<SubscriptionReviewSummary> {
    let rows = sqlx::query_as::<_, ReviewCountRow>(
        "SELECT s.id AS subscription_id, \
                SUM(CASE WHEN i.state = 'pending' THEN 1 ELSE 0 END) AS pending \
         FROM subscriptions s \
         LEFT JOIN subscription_items i ON i.subscription_id = s.id \
         WHERE s.kind = 'indexer' GROUP BY s.id ORDER BY s.name, s.id",
    )
    .fetch_all(pool)
    .await?;
    let subscriptions = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<SubscriptionReviewCount>>>()?;
    let pending_total = subscriptions.iter().map(|entry| entry.pending).sum();
    Ok(SubscriptionReviewSummary {
        pending_total,
        subscriptions,
    })
}

/// Snapshot of all items that are pending at the start of a bulk review action.
pub(crate) async fn pending_item_ids(
    pool: &SqlitePool,
    id: SubscriptionId,
) -> Result<Vec<SubscriptionItemId>> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM subscription_items WHERE subscription_id = ? AND state = 'pending' \
         ORDER BY discovered_at, id",
    )
    .bind(id.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|value| Ok(SubscriptionItemId::from_uuid(value.parse()?)))
    .collect()
}

/// One item by id, for the review actions that act on a single row.
pub(crate) async fn item(
    pool: &SqlitePool,
    id: rd_core::SubscriptionItemId,
) -> Result<Option<SubscriptionItem>> {
    sqlx::query_as::<_, ItemRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {ITEM_COLUMNS} FROM subscription_items WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn runs(
    pool: &SqlitePool,
    id: SubscriptionId,
    limit: i64,
) -> Result<Vec<SubscriptionRun>> {
    sqlx::query_as::<_, RunRow>(
        "SELECT id, subscription_id, started_at, finished_at, found, accepted, skipped, error \
         FROM subscription_runs WHERE subscription_id = ? ORDER BY started_at DESC LIMIT ?",
    )
    .bind(id.to_string())
    .bind(limit)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Whether the subscription's archive holds an item under `key` (RD-1150-05): where an indexer
/// poll meets what it already has. One lookup on the UNIQUE index over `(subscription_id,
/// item_key)`.
pub(crate) async fn knows_item(pool: &SqlitePool, id: SubscriptionId, key: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM subscription_items WHERE subscription_id = ? AND item_key = ?)",
    )
    .bind(id.to_string())
    .bind(key)
    .fetch_one(pool)
    .await?
        != 0)
}

const fn kind_string(kind: SubscriptionKind) -> &'static str {
    match kind {
        SubscriptionKind::Media => "media",
        SubscriptionKind::Gallery => "gallery",
        SubscriptionKind::Feed => "feed",
        SubscriptionKind::Indexer => "indexer",
        SubscriptionKind::SiteRule => "site_rule",
        SubscriptionKind::Script => "script",
        SubscriptionKind::GitRelease => "git_release",
    }
}

const fn mode_string(mode: SubscriptionMode) -> &'static str {
    match mode {
        SubscriptionMode::Review => "review",
        SubscriptionMode::AutoQueue => "auto_queue",
    }
}

const fn item_state_string(state: SubscriptionItemState) -> &'static str {
    match state {
        SubscriptionItemState::Pending => "pending",
        SubscriptionItemState::Queued => "queued",
        SubscriptionItemState::Skipped => "skipped",
        SubscriptionItemState::Dismissed => "dismissed",
    }
}

const fn reason_string(reason: FilterReason) -> &'static str {
    match reason {
        FilterReason::TitleNotIncluded => "title_not_included",
        FilterReason::TitleExcluded => "title_excluded",
        FilterReason::TooShort => "too_short",
        FilterReason::TooLong => "too_long",
        FilterReason::TooOld => "too_old",
        FilterReason::LanguageNotWanted => "language_not_wanted",
        FilterReason::ResolutionTooLow => "resolution_too_low",
        FilterReason::Backlog => "backlog",
        FilterReason::AssetNotWanted => "asset_not_wanted",
    }
}
