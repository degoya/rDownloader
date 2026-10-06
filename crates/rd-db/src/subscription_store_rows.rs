//! The row types of the subscription tables and their conversion into the domain types.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_core::{
    CategoryId, DownloadPriority, FilterReason, Subscription, SubscriptionCardRatio,
    SubscriptionId, SubscriptionItem, SubscriptionItemCounts, SubscriptionItemId,
    SubscriptionItemState, SubscriptionKind, SubscriptionMode, SubscriptionReviewCount,
    SubscriptionRun, SubscriptionRunId, SubscriptionView,
};
use sqlx::FromRow;
use url::Url;

use crate::json_column::lenient;

#[derive(FromRow)]
pub(super) struct SubscriptionRow {
    id: String,
    name: String,
    url: String,
    kind: String,
    enabled: i64,
    mode: String,
    category_id: Option<String>,
    priority: i64,
    interval_seconds: i64,
    filters_json: Option<String>,
    backlog_json: Option<String>,
    category_map_json: Option<String>,
    source_categories_json: Option<String>,
    every_release: i64,
    view: String,
    autoplay: i64,
    card_ratio: String,
    schedule: Option<String>,
    script_arguments_json: String,
    indexer_search_json: String,
    git_release_json: String,
    primed: i64,
    last_run_at: Option<DateTime<Utc>>,
    next_run_at: Option<DateTime<Utc>>,
    consecutive_failures: i64,
    last_error: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    secret_ref: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<SubscriptionRow> for Subscription {
    type Error = anyhow::Error;

    fn try_from(row: SubscriptionRow) -> Result<Self> {
        Ok(Self {
            id: SubscriptionId::from_uuid(row.id.parse()?),
            name: row.name,
            url: Url::parse(&row.url)?,
            kind: match row.kind.as_str() {
                "gallery" => SubscriptionKind::Gallery,
                "feed" => SubscriptionKind::Feed,
                "indexer" => SubscriptionKind::Indexer,
                "site_rule" => SubscriptionKind::SiteRule,
                "script" => SubscriptionKind::Script,
                "git_release" => SubscriptionKind::GitRelease,
                _ => SubscriptionKind::Media,
            },
            enabled: row.enabled != 0,
            mode: if row.mode == "auto_queue" {
                SubscriptionMode::AutoQueue
            } else {
                SubscriptionMode::Review
            },
            category_id: row
                .category_id
                .map(|value| value.parse().map(CategoryId::from_uuid))
                .transpose()?,
            priority: DownloadPriority::from_i32(i32::try_from(row.priority).unwrap_or_default()),
            interval_seconds: u32::try_from(row.interval_seconds).unwrap_or_default(),
            // The one field that must not fall back: an empty filter set means "accept
            // everything", so a blob that fails to parse would silently turn an exclusion
            // filter into a subscription that queues the entire feed. Fail the row instead —
            // a corrupt `url` or `id` already does, and a subscription nobody can read must
            // not poll. A NULL column is not corruption: it predates the column and never
            // filtered anything.
            filters: match row.filters_json.as_deref() {
                None => Default::default(),
                Some(value) => serde_json::from_str(value).map_err(|error| {
                    tracing::warn!(
                        subscription = %row.id,
                        %error,
                        "subscription filters are unreadable; the subscription is refused \
                         rather than polled without them"
                    );
                    anyhow::Error::new(error).context("subscription filters are unreadable")
                })?,
            },
            // A blob written by a newer version deserialises as far as it can and the rest
            // defaults, rather than making the whole subscription unreadable.
            backlog: row
                .backlog_json
                .as_deref()
                .and_then(|value| {
                    lenient(
                        serde_json::from_str(value),
                        "subscriptions",
                        "backlog_json",
                        &row.id,
                    )
                })
                .unwrap_or_default(),
            category_map: row
                .category_map_json
                .as_deref()
                .and_then(|value| {
                    lenient(
                        serde_json::from_str(value),
                        "subscriptions",
                        "category_map_json",
                        &row.id,
                    )
                })
                .unwrap_or_default(),
            // NULL for every row written before the column existed; empty means "everything",
            // so an older subscription keeps asking for exactly what it always did.
            source_categories: row
                .source_categories_json
                .as_deref()
                .and_then(|value| {
                    lenient(
                        serde_json::from_str(value),
                        "subscriptions",
                        "source_categories_json",
                        &row.id,
                    )
                })
                .unwrap_or_default(),
            primed: row.primed != 0,
            last_run_at: row.last_run_at,
            next_run_at: row.next_run_at,
            consecutive_failures: u32::try_from(row.consecutive_failures).unwrap_or_default(),
            last_error: row.last_error,
            etag: row.etag,
            last_modified: row.last_modified,
            has_secret: row.secret_ref.is_some(),
            secret_ref: row.secret_ref,
            every_release: row.every_release != 0,
            view: SubscriptionView::from_stored(&row.view),
            autoplay: row.autoplay != 0,
            card_ratio: SubscriptionCardRatio::from_stored(&row.card_ratio),
            schedule: row.schedule,
            // Refused rather than defaulted, like the filters: a script run without the
            // arguments it was given runs another variant of it, which nobody asked for.
            script_arguments: serde_json::from_str(&row.script_arguments_json)
                .context("subscription script arguments are unreadable")?,
            // Refused as well: a search without its term or its age limit asks the indexer for
            // something else than the person configured (RD-180-20).
            indexer_search: serde_json::from_str(&row.indexer_search_json)
                .context("subscription search parameters are unreadable")?,
            // Refused as well: a subscription that lost its platform filter would download
            // every file of every release (RD-190-13).
            git_release: serde_json::from_str(&row.git_release_json)
                .context("subscription git-release options are unreadable")?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
pub(super) struct ItemRow {
    id: String,
    subscription_id: String,
    item_key: String,
    title: String,
    url: String,
    published_at: Option<DateTime<Utc>>,
    duration_seconds: Option<i64>,
    state: String,
    reason: Option<String>,
    source_category: Option<String>,
    media_type: Option<String>,
    attributes_json: Option<String>,
    discovered_at: DateTime<Utc>,
}

#[derive(FromRow)]
pub(super) struct ItemCountRow {
    pending: i64,
    queued: i64,
    skipped: i64,
    dismissed: i64,
}

impl From<ItemCountRow> for SubscriptionItemCounts {
    fn from(row: ItemCountRow) -> Self {
        Self {
            pending: u64::try_from(row.pending).unwrap_or_default(),
            queued: u64::try_from(row.queued).unwrap_or_default(),
            skipped: u64::try_from(row.skipped).unwrap_or_default(),
            dismissed: u64::try_from(row.dismissed).unwrap_or_default(),
        }
    }
}

#[derive(FromRow)]
pub(super) struct ReviewCountRow {
    subscription_id: String,
    pending: i64,
}

impl TryFrom<ReviewCountRow> for SubscriptionReviewCount {
    type Error = anyhow::Error;

    fn try_from(row: ReviewCountRow) -> Result<Self> {
        Ok(Self {
            subscription_id: SubscriptionId::from_uuid(row.subscription_id.parse()?),
            pending: u64::try_from(row.pending).unwrap_or_default(),
        })
    }
}

impl TryFrom<ItemRow> for SubscriptionItem {
    type Error = anyhow::Error;

    fn try_from(row: ItemRow) -> Result<Self> {
        Ok(Self {
            id: SubscriptionItemId::from_uuid(row.id.parse()?),
            subscription_id: SubscriptionId::from_uuid(row.subscription_id.parse()?),
            item_key: row.item_key,
            title: row.title,
            url: Url::parse(&row.url)?,
            published_at: row.published_at,
            duration_seconds: row
                .duration_seconds
                .and_then(|value| u32::try_from(value).ok()),
            state: match row.state.as_str() {
                "queued" => SubscriptionItemState::Queued,
                "skipped" => SubscriptionItemState::Skipped,
                "dismissed" => SubscriptionItemState::Dismissed,
                _ => SubscriptionItemState::Pending,
            },
            reason: row.reason.as_deref().and_then(|value| match value {
                "title_not_included" => Some(FilterReason::TitleNotIncluded),
                "title_excluded" => Some(FilterReason::TitleExcluded),
                "too_short" => Some(FilterReason::TooShort),
                "too_long" => Some(FilterReason::TooLong),
                "too_old" => Some(FilterReason::TooOld),
                "language_not_wanted" => Some(FilterReason::LanguageNotWanted),
                "resolution_too_low" => Some(FilterReason::ResolutionTooLow),
                "backlog" => Some(FilterReason::Backlog),
                "asset_not_wanted" => Some(FilterReason::AssetNotWanted),
                _ => None,
            }),
            source_category: row.source_category,
            media_type: row.media_type,
            // A row written before RD-101-17, or one whose stored JSON no longer parses, is
            // an item without details rather than a row that fails to load.
            attributes: row
                .attributes_json
                .as_deref()
                .and_then(|value| {
                    lenient(
                        serde_json::from_str(value),
                        "subscription_items",
                        "attributes_json",
                        &row.id,
                    )
                })
                .unwrap_or_default(),
            // In the vault (RD-190-04); revealed for the answers that show it.
            password: None,
            discovered_at: row.discovered_at,
        })
    }
}

#[derive(FromRow)]
pub(super) struct RunRow {
    id: String,
    subscription_id: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    found: i64,
    accepted: i64,
    skipped: i64,
    error: Option<String>,
}

impl TryFrom<RunRow> for SubscriptionRun {
    type Error = anyhow::Error;

    fn try_from(row: RunRow) -> Result<Self> {
        Ok(Self {
            id: SubscriptionRunId::from_uuid(row.id.parse()?),
            subscription_id: SubscriptionId::from_uuid(row.subscription_id.parse()?),
            started_at: row.started_at,
            finished_at: row.finished_at,
            found: u32::try_from(row.found).unwrap_or_default(),
            accepted: u32::try_from(row.accepted).unwrap_or_default(),
            skipped: u32::try_from(row.skipped).unwrap_or_default(),
            error: row.error,
        })
    }
}
