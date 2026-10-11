//! Whether a package may download now (RD-1240-30): its download window, and the bandwidth
//! schedule's profile that pauses downloads.
//!
//! The windows are read in the schedule's timezone by the rule its own windows follow
//! ([`covers_local`]), so a window that wraps midnight or meets a daylight-saving change behaves
//! exactly like a schedule window would.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use rd_core::DownloadWindow;
use serde::Serialize;
use utoipa::ToSchema;

use crate::schedule::{DaySet, covers_local, local_position};

/// Why a package's files wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadHold {
    /// Its own or its category's download window is closed.
    Window,
    /// The active bandwidth profile pauses downloads, and the package does not ignore that.
    Schedule,
}

/// Whether `window` lets its package download at `now`; a window without spans always does.
#[must_use]
pub fn window_open(window: &DownloadWindow, timezone: Tz, now: DateTime<Utc>) -> bool {
    if window.windows.is_empty() {
        return true;
    }
    let (weekday, minute) = local_position(timezone, now);
    window.windows.iter().any(|span| {
        covers_local(
            DaySet(span.days),
            span.start_minute,
            span.end_minute,
            weekday,
            minute,
        )
    })
}

/// What holds a package back at `now`, if anything.
///
/// `window` is the one that applies to the package (its own, else its category's), and
/// `schedule_pauses` whether the active bandwidth profile pauses downloads. A closed window
/// holds the package whatever the schedule says; the schedule's pause holds it unless the
/// window says to ignore it.
#[must_use]
pub fn package_hold(
    window: Option<&DownloadWindow>,
    schedule_pauses: bool,
    timezone: Tz,
    now: DateTime<Utc>,
) -> Option<DownloadHold> {
    if window.is_some_and(|window| !window_open(window, timezone, now)) {
        return Some(DownloadHold::Window);
    }
    let ignores = window.is_some_and(|window| window.ignore_schedule_pause);
    (schedule_pauses && !ignores).then_some(DownloadHold::Schedule)
}

#[cfg(test)]
#[path = "download_window_tests.rs"]
mod tests;
