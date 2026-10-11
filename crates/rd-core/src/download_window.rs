//! A package's or a category's download window (RD-1240-30): the weekly times its files may
//! download, and whether they keep downloading while the bandwidth schedule pauses downloads.
//!
//! Only the shape lives here, shared by the package, the category, the database and the REST
//! layer; whether a window is open at an instant is `rd-limits`' to say, in the bandwidth
//! schedule's timezone and by the same rule its windows follow.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The most weekly spans one download window carries; four a day for a week is plenty.
pub const MAX_DOWNLOAD_WINDOW_SPANS: usize = 28;

/// One weekly span of a download window, local times in the bandwidth schedule's timezone.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct WeeklyWindow {
    /// Monday-first bitmask; bit 0 = Monday.
    pub days: u8,
    /// Minutes since local midnight.
    pub start_minute: u16,
    /// Exclusive; below `start_minute` the span wraps past midnight and belongs to the day it
    /// starts on, so "Fri 22:00–06:00" still holds at Saturday 05:00.
    pub end_minute: u16,
}

/// When a package's files may download, set on the package or, as the default of its packages,
/// on a category. A package's own setting wins over its category's as a whole; neither means the
/// package follows the bandwidth schedule only.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct DownloadWindow {
    /// The times the files may download; outside them waiting files wait and running resumable
    /// transfers pause. Empty means at any time, so only `ignore_schedule_pause` counts.
    pub windows: Vec<WeeklyWindow>,
    /// Downloads even while the bandwidth schedule's active profile pauses downloads. Every rate
    /// limit still applies: a package is never faster than the global, profile or hand-set limit.
    pub ignore_schedule_pause: bool,
}

impl DownloadWindow {
    /// The window that applies to a package: its own, otherwise its category's.
    #[must_use]
    pub fn effective<'a>(
        package: Option<&'a Self>,
        category: Option<&'a Self>,
    ) -> Option<&'a Self> {
        package.or(category)
    }
}

#[cfg(test)]
mod tests {
    use super::{DownloadWindow, WeeklyWindow};

    #[test]
    fn a_package_setting_wins_over_its_category_as_a_whole() {
        let night = DownloadWindow {
            windows: vec![WeeklyWindow {
                days: 0b0111_1111,
                start_minute: 22 * 60,
                end_minute: 6 * 60,
            }],
            ignore_schedule_pause: false,
        };
        // A package's own setting without spans opts out of the category's window.
        let anytime = DownloadWindow::default();
        assert_eq!(
            DownloadWindow::effective(Some(&anytime), Some(&night)),
            Some(&anytime)
        );
        assert_eq!(DownloadWindow::effective(None, Some(&night)), Some(&night));
        assert_eq!(DownloadWindow::effective(None, None), None);
    }

    #[test]
    fn a_stored_window_without_the_switch_obeys_the_schedule() {
        let window: DownloadWindow =
            serde_json::from_str(r#"{"windows":[{"days":1,"start_minute":0,"end_minute":60}]}"#)
                .expect("window");
        assert!(!window.ignore_schedule_pause);
        assert_eq!(window.windows.len(), 1);
    }
}
