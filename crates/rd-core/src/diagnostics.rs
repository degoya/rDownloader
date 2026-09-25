//! The contract of the structured log store and the diagnostic bundle (RD-110-02).
//!
//! Only the vocabulary lives here — the level a record carries and the retention a person can
//! set — because `rd-db` stores it, `rd-diagnostics` fills it and `rd-api` serves it, and each
//! of those would otherwise keep its own copy of the same five words.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The severity of one stored log record, in `tracing`'s order.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Every level, least severe first.
    pub const ALL: [Self; 5] = [
        Self::Trace,
        Self::Debug,
        Self::Info,
        Self::Warn,
        Self::Error,
    ];

    /// The lowercase word the store and the API use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    /// Parses the lowercase word; anything else is `None` rather than a default, so a filter
    /// with a typo refuses instead of silently showing everything.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|level| level.as_str().eq_ignore_ascii_case(value.trim()))
    }

    /// This level and every more severe one — what a "show warnings" filter means.
    #[must_use]
    pub fn and_above(self) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|level| *level >= self)
            .collect()
    }
}

/// How much of the log store is kept.
///
/// A slice of the `service.settings` blob (see `rd_db::Database::service_settings_or_default`),
/// so the fields are named exactly as they appear in the settings document.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct LogRetentionSettings {
    /// Records kept at most; the oldest go first.
    pub log_retention_records: u32,
    /// Days a record is kept at most, whatever the count.
    pub log_retention_days: u32,
}

/// What a fresh installation keeps: two weeks or twenty thousand records, whichever runs out
/// first. Enough to cover a problem noticed on Monday that happened on the weekend, small
/// enough that the sweep never has much to do.
impl Default for LogRetentionSettings {
    fn default() -> Self {
        Self {
            log_retention_records: DEFAULT_LOG_RETENTION_RECORDS,
            log_retention_days: DEFAULT_LOG_RETENTION_DAYS,
        }
    }
}

pub const DEFAULT_LOG_RETENTION_RECORDS: u32 = 20_000;
pub const DEFAULT_LOG_RETENTION_DAYS: u32 = 14;
/// The count a person may set. The floor keeps the viewer useful; the ceiling keeps the
/// database file and every sweep bounded.
pub const LOG_RETENTION_RECORDS_RANGE: RangeInclusive<u32> = 1_000..=500_000;
pub const LOG_RETENTION_DAYS_RANGE: RangeInclusive<u32> = 1..=365;

#[cfg(test)]
mod tests {
    use super::{LogLevel, LogRetentionSettings};

    #[test]
    fn levels_parse_their_own_word_and_nothing_else() {
        assert_eq!(LogLevel::parse("WARN"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse(" error "), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("warning"), None);
        assert_eq!(LogLevel::parse(""), None);
    }

    #[test]
    fn and_above_is_this_level_and_the_more_severe_ones() {
        assert_eq!(
            LogLevel::Warn.and_above(),
            vec![LogLevel::Warn, LogLevel::Error]
        );
        assert_eq!(LogLevel::Trace.and_above().len(), 5);
    }

    #[test]
    fn a_missing_field_reads_as_the_default() {
        let parsed: LogRetentionSettings =
            serde_json::from_value(serde_json::json!({ "log_retention_days": 3 })).expect("slice");
        assert_eq!(parsed.log_retention_days, 3);
        assert_eq!(parsed.log_retention_records, 20_000);
    }
}
