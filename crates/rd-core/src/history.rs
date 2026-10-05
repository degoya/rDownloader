//! The persistent download history (RD-1100-04): what was downloaded, kept after the package
//! left the queue.
//!
//! One entry per package, written in the transaction that gives the package its outcome, so a
//! package that is removed or cleaned up afterwards is still findable by name. The history is
//! neither the statistics nor the audit log: it holds no figures over time and no actor, only
//! what a person searches for later — the name, where it came from, where it went and how it
//! ended.

use std::ops::RangeInclusive;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use utoipa::ToSchema;

use crate::{ByteCount, DownloadKind, MessageParams, PackageId};

/// How a package ended.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOutcome {
    Completed,
    Failed,
}

/// One package the history remembers.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct HistoryEntry {
    pub id: i64,
    /// The package this entry describes; it may no longer be in the queue.
    pub package_id: PackageId,
    pub name: String,
    pub kind: DownloadKind,
    /// The category's name when the package ended, if it had one.
    pub category: Option<String>,
    pub destination: String,
    pub total_bytes: ByteCount,
    pub file_count: u32,
    /// The addresses the files came from, with every credential masked
    /// ([`history_source`]). Empty for a package whose files had no address of their own (an
    /// imported NZB); such an entry cannot be added again.
    pub sources: Vec<String>,
    pub outcome: HistoryOutcome,
    /// The stable code of the failure, for `failed`.
    pub error_code: Option<String>,
    /// The parameters that code is translated with, already redacted.
    #[serde(default, skip_serializing_if = "MessageParams::is_empty")]
    pub error_params: MessageParams,
    /// When the package was added to the queue.
    pub created_at: DateTime<Utc>,
    /// When it reached its outcome.
    pub finished_at: DateTime<Utc>,
}

/// The most addresses one entry keeps. A package of thousands of links still finds by name;
/// re-adding it takes the first of them.
pub const HISTORY_MAX_SOURCES: usize = 100;

/// The address as the history keeps it, or `None` when it is not one a person could add again.
///
/// The shared masking of every credential-bearing query value and any userinfo
/// ([`crate::redact_url`]), and the fragment dropped as well: a declaring provider's fragment
/// is a key (RD-110-38), and the history is no place for one. The `nzb://` addresses of an
/// imported NZB's files name rows of this database, not anything on the network.
#[must_use]
pub fn history_source(url: &Url) -> Option<String> {
    if url.scheme() == "nzb" {
        return None;
    }
    let mut url = url.clone();
    url.set_fragment(None);
    Some(crate::redact_url(&url))
}

/// How much of the history is kept.
///
/// A slice of the `service.settings` blob, named exactly as the fields appear in the settings
/// document. The sweep that applies it runs with the log and audit retention.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct HistoryRetentionSettings {
    /// Entries kept at most; the oldest go first.
    pub history_retention_entries: u32,
    /// Days an entry is kept at most, whatever the count.
    pub history_retention_days: u32,
}

impl Default for HistoryRetentionSettings {
    fn default() -> Self {
        Self {
            history_retention_entries: DEFAULT_HISTORY_RETENTION_ENTRIES,
            history_retention_days: DEFAULT_HISTORY_RETENTION_DAYS,
        }
    }
}

pub const DEFAULT_HISTORY_RETENTION_ENTRIES: u32 = 10_000;
pub const DEFAULT_HISTORY_RETENTION_DAYS: u32 = 365;
pub const HISTORY_RETENTION_ENTRIES_RANGE: RangeInclusive<u32> = 100..=100_000;
pub const HISTORY_RETENTION_DAYS_RANGE: RangeInclusive<u32> = 1..=3650;

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{HistoryRetentionSettings, history_source};

    #[test]
    fn a_source_loses_its_credentials_and_its_fragment() {
        let url = Url::parse(
            "https://user:canary-password@files.example/get?id=7&token=canary-token#canary-key",
        )
        .expect("url");
        let kept = history_source(&url).expect("kept");
        assert!(!kept.contains("canary"), "{kept}");
        assert!(kept.contains("id=7"), "{kept}");
    }

    #[test]
    fn an_nzb_file_address_is_not_kept() {
        let url = Url::parse("nzb://import/file").expect("url");
        assert_eq!(history_source(&url), None);
    }

    #[test]
    fn a_missing_retention_field_reads_as_the_default() {
        let parsed: HistoryRetentionSettings =
            serde_json::from_value(serde_json::json!({ "history_retention_days": 30 }))
                .expect("slice");
        assert_eq!(parsed.history_retention_days, 30);
        assert_eq!(parsed.history_retention_entries, 10_000);
    }
}
