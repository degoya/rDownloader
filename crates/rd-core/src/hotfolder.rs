//! How often a watched folder is looked at (RD-110-31).

use std::{ops::RangeInclusive, time::Duration};

use serde::{Deserialize, Serialize};

/// The reconciliation interval every hotfolder watcher shares.
///
/// A slice of the `service.settings` blob (see `rd_db::Database::service_settings_or_default`),
/// so the field is named exactly as it appears in the settings document. One value for all
/// folders on purpose: the finding was "30 seconds is the wrong number", not "folder A differs
/// from folder B", and a per-folder value would cost a column and a migration nobody asked for.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct HotFolderSettings {
    /// Seconds between two reconciliation scans of every watched folder.
    pub hotfolder_poll_seconds: u32,
}

impl Default for HotFolderSettings {
    fn default() -> Self {
        Self {
            hotfolder_poll_seconds: DEFAULT_HOTFOLDER_POLL_SECONDS,
        }
    }
}

impl HotFolderSettings {
    /// The interval as the watcher needs it.
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(u64::from(self.hotfolder_poll_seconds))
    }
}

/// What a fresh installation polls at, and what every installation polled at before the value
/// could be set.
pub const DEFAULT_HOTFOLDER_POLL_SECONDS: u32 = 30;
/// The seconds a person may set. The floor keeps a large recursive tree from being scanned
/// without pause; the ceiling is once an hour, beyond which the native watcher is all there is.
pub const HOTFOLDER_POLL_SECONDS_RANGE: RangeInclusive<u32> = 5..=3600;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{HOTFOLDER_POLL_SECONDS_RANGE, HotFolderSettings};

    #[test]
    fn a_missing_field_reads_as_thirty_seconds() {
        let parsed: HotFolderSettings =
            serde_json::from_value(serde_json::json!({ "log_retention_days": 3 })).expect("slice");
        assert_eq!(parsed.hotfolder_poll_seconds, 30);
        assert_eq!(parsed.poll_interval(), Duration::from_secs(30));
    }

    #[test]
    fn a_present_field_is_read_as_seconds() {
        let parsed: HotFolderSettings =
            serde_json::from_value(serde_json::json!({ "hotfolder_poll_seconds": 45 }))
                .expect("slice");
        assert_eq!(parsed.poll_interval(), Duration::from_secs(45));
    }

    #[test]
    fn the_default_sits_inside_the_range() {
        assert!(
            HOTFOLDER_POLL_SECONDS_RANGE
                .contains(&HotFolderSettings::default().hotfolder_poll_seconds)
        );
        assert_eq!(*HOTFOLDER_POLL_SECONDS_RANGE.start(), 5);
        assert_eq!(*HOTFOLDER_POLL_SECONDS_RANGE.end(), 3600);
    }
}
