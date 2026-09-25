//! What polling recorded: the item archive, its pages and counts, and the poll runs.
//!
//! Split out of `subscription.rs` for size (RD-110-37); the types are unchanged and are
//! re-exported from the parent module, so every path into them stayed the same.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{SubscriptionId, SubscriptionItemId, SubscriptionRunId};

use super::FilterReason;

/// What happened to one discovered item.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionItemState {
    /// Waiting for a person to decide (review mode, or a backlog collection).
    #[default]
    Pending,
    /// Handed to intake.
    Queued,
    /// Rejected by a filter or the backlog cutoff; kept so it is never reconsidered.
    Skipped,
    /// Dismissed by a person.
    Dismissed,
}

/// One item a poll found.
///
/// The row exists whatever the outcome — accepted, filtered or skipped as backlog — because
/// its presence is what stops the next poll from looking at it again. A skipped item that
/// were simply not stored would be rediscovered forever.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SubscriptionItem {
    pub id: SubscriptionItemId,
    pub subscription_id: SubscriptionId,
    /// Canonical, source-stable identity. Unique per subscription.
    pub item_key: String,
    pub title: String,
    #[schema(value_type = String, format = "uri")]
    pub url: Url,
    pub published_at: Option<DateTime<Utc>>,
    pub duration_seconds: Option<u32>,
    pub state: SubscriptionItemState,
    /// Which rule rejected it, when one did.
    pub reason: Option<FilterReason>,
    /// The media type the source declared for `url`, when it declared one.
    ///
    /// What an indexer hands over is not visible in its download address, so this is what
    /// decides whether the link is imported as an NZB or a torrent rather than fetched as an
    /// ordinary file.
    #[serde(default)]
    pub media_type: Option<String>,
    /// The category the source assigned, kept raw so a mapping added later still applies to
    /// items that were archived before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_category: Option<String>,
    /// What the source said *about* the release (RD-101-17): cover address, ids, resolution,
    /// size, and whatever else the indexer chose to emit.
    ///
    /// Already filtered by `rd_subscription::retain_attributes` before it was stored, so a
    /// value here is safe to render; the `password` key is the specification's flag
    /// (`1` protected), never the secret itself.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
    /// The archive password the source announced, when it announced one.
    ///
    /// Deliberately readable like `DownloadPackage::password`: release passwords come from the
    /// title, feed or NZB rather than from an account credential, and seeing the exact value is
    /// what lets a person diagnose an extraction failure. The subscription routes still require
    /// an authenticated session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub discovered_at: DateTime<Utc>,
}

/// Counts of every item state in one subscription, independent of the selected page filter.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SubscriptionItemCounts {
    pub pending: u64,
    pub queued: u64,
    pub skipped: u64,
    pub dismissed: u64,
}

/// One server-filtered page of a subscription's item archive.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SubscriptionItemPage {
    pub items: Vec<SubscriptionItem>,
    /// Number of rows matching the selected state filter.
    pub total: u64,
    /// State totals across the entire subscription, not just this page.
    pub counts: SubscriptionItemCounts,
    /// Number of recorded checks that the history cleanup would remove.
    pub run_total: u64,
}

/// Pending review count for one indexer subscription.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SubscriptionReviewCount {
    pub subscription_id: SubscriptionId,
    pub pending: u64,
}

/// Lightweight indexer-review summary used before any item page is expanded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SubscriptionReviewSummary {
    pub pending_total: u64,
    pub subscriptions: Vec<SubscriptionReviewCount>,
}

/// Result of setting every item that was pending when a bulk action started.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SubscriptionBulkStateResponse {
    pub matched: u64,
    pub updated: u64,
    pub failed: u64,
}

/// Rows removed by a per-subscription history cleanup.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct SubscriptionHistoryClearResponse {
    pub deleted_items: u64,
    pub deleted_runs: u64,
}

/// One completed poll, kept as history so a silent subscription can be told apart from a
/// failing one.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SubscriptionRun {
    pub id: SubscriptionRunId,
    pub subscription_id: SubscriptionId,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Items the source offered.
    pub found: u32,
    /// Items that were new *and* passed the filters.
    pub accepted: u32,
    /// New items a filter or the backlog cutoff rejected.
    pub skipped: u32,
    /// Redacted failure message; `None` for a successful poll.
    pub error: Option<String>,
}
