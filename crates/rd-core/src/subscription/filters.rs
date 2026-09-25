//! The filter set a discovered item is judged against, and the reasons it can be refused.
//!
//! Split out of `subscription.rs` for size (RD-110-37); the types are unchanged and are
//! re-exported from the parent module, so every path into them stayed the same.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Filters applied to a discovered item, in the order the UI lists them.
///
/// Every field is optional and an empty one means "no opinion", so a filter set written by
/// an older version still deserialises and still means the same thing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct SubscriptionFilters {
    /// Case-insensitive substrings; an item must contain at least one when non-empty.
    pub title_contains: Vec<String>,
    /// Case-insensitive substrings; an item containing any of them is rejected. Applied
    /// after `title_contains`, so an exclusion always wins.
    pub title_excludes: Vec<String>,
    pub min_duration_seconds: Option<u32>,
    pub max_duration_seconds: Option<u32>,
    /// Rejects items published before this instant, independently of the backlog policy.
    pub published_after: Option<DateTime<Utc>>,
    /// Accepted language tags, compared case-insensitively on the primary subtag so `en`
    /// matches `en-GB`.
    pub languages: Vec<String>,
    /// Minimum vertical resolution, where the source reports one.
    pub min_height: Option<u32>,
}

/// Why an item was not accepted.
///
/// A variant per rule rather than a message, so the UI can translate the reason and a test
/// can assert on it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterReason {
    TitleNotIncluded,
    TitleExcluded,
    TooShort,
    TooLong,
    TooOld,
    LanguageNotWanted,
    ResolutionTooLow,
    /// Older than the backlog cutoff chosen when the subscription was switched on.
    Backlog,
}

impl FilterReason {
    /// Stable key the UI translates, following the `<domain>.<subject>` convention.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::TitleNotIncluded => "subscription.filter.title_not_included",
            Self::TitleExcluded => "subscription.filter.title_excluded",
            Self::TooShort => "subscription.filter.too_short",
            Self::TooLong => "subscription.filter.too_long",
            Self::TooOld => "subscription.filter.too_old",
            Self::LanguageNotWanted => "subscription.filter.language_not_wanted",
            Self::ResolutionTooLow => "subscription.filter.resolution_too_low",
            Self::Backlog => "subscription.filter.backlog",
        }
    }
}
