//! Quiet hours: weekly windows during which resource-intensive work waits.
//!
//! Shares the schedule's evaluation, so quiet hours behave the same way across a
//! daylight-saving change as the bandwidth windows do.

use jiff::tz::TimeZone;
use rd_core::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::schedule::{DaySet, covers_local, floor_to_minute, local_position};

/// One quiet window; local times in the configured timezone.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct QuietWindow {
    /// Monday-first bitmask; bit 0 = Monday.
    pub days: DaySet,
    pub start_minute: u16,
    /// Exclusive; below `start_minute` the window wraps past midnight.
    pub end_minute: u16,
}

/// The configured quiet hours.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(default)]
pub struct QuietHours {
    pub enabled: bool,
    pub windows: Vec<QuietWindow>,
}

impl QuietHours {
    /// Whether `now` falls inside a quiet window.
    #[must_use]
    pub fn is_quiet(&self, timezone: &TimeZone, now: Timestamp) -> bool {
        if !self.enabled {
            return false;
        }
        let (weekday, minute) = local_position(timezone, now);
        self.windows.iter().any(|window| {
            covers_local(
                window.days,
                window.start_minute,
                window.end_minute,
                weekday,
                minute,
            )
        })
    }

    /// When the current quiet period ends, so deferred work can say how long it waits.
    #[must_use]
    pub fn ends_after(&self, timezone: &TimeZone, now: Timestamp) -> Option<Timestamp> {
        if !self.is_quiet(timezone, now) {
            return None;
        }
        let mut cursor = floor_to_minute(now);
        let deadline = now + SignedDuration::from_hours(24 * 8);
        while cursor < deadline {
            cursor += SignedDuration::from_mins(1);
            if !self.is_quiet(timezone, cursor) {
                return Some(cursor);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;
    use rd_core::Timestamp;

    use super::{DaySet, QuietHours, QuietWindow};

    fn berlin() -> TimeZone {
        TimeZone::get("Europe/Berlin").expect("zone")
    }

    fn nightly() -> QuietHours {
        QuietHours {
            enabled: true,
            windows: vec![QuietWindow {
                days: DaySet::EVERY_DAY,
                start_minute: 23 * 60,
                end_minute: 7 * 60,
            }],
        }
    }

    #[test]
    fn a_wrapping_quiet_window_covers_both_sides_of_midnight() {
        let quiet = nightly();
        let zone = &berlin();
        // 23:30 Berlin = 22:30 UTC in winter.
        assert!(quiet.is_quiet(
            zone,
            Timestamp::from_ymd_hms(2026, 1, 15, 22, 30, 0).expect("a valid instant")
        ));
        // 02:00 Berlin the next morning.
        assert!(quiet.is_quiet(
            zone,
            Timestamp::from_ymd_hms(2026, 1, 16, 1, 0, 0).expect("a valid instant")
        ));
        // 12:00 Berlin is not quiet.
        assert!(!quiet.is_quiet(
            zone,
            Timestamp::from_ymd_hms(2026, 1, 16, 11, 0, 0).expect("a valid instant")
        ));
    }

    #[test]
    fn disabled_quiet_hours_never_defer_anything() {
        let mut quiet = nightly();
        quiet.enabled = false;
        assert!(!quiet.is_quiet(
            &berlin(),
            Timestamp::from_ymd_hms(2026, 1, 15, 22, 30, 0).expect("a valid instant")
        ));
    }

    #[test]
    fn the_end_of_the_quiet_period_is_reported_in_utc() {
        let quiet = nightly();
        let zone = &berlin();
        let inside = Timestamp::from_ymd_hms(2026, 1, 16, 1, 0, 0).expect("a valid instant");
        // 07:00 Berlin = 06:00 UTC in winter.
        assert_eq!(
            quiet.ends_after(zone, inside),
            Some(Timestamp::from_ymd_hms(2026, 1, 16, 6, 0, 0).expect("a valid instant"))
        );
        let outside = Timestamp::from_ymd_hms(2026, 1, 16, 11, 0, 0).expect("a valid instant");
        assert!(quiet.ends_after(zone, outside).is_none());
    }
}
