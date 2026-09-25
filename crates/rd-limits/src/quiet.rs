//! Quiet hours: weekly windows during which resource-intensive work waits.
//!
//! Shares the schedule's evaluation, so quiet hours behave the same way across a
//! daylight-saving change as the bandwidth windows do.

use chrono::{DateTime, Duration, Timelike, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::schedule::{DaySet, covers_local, local_position};

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
    pub fn is_quiet(&self, timezone: Tz, now: DateTime<Utc>) -> bool {
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
    pub fn ends_after(&self, timezone: Tz, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        if !self.is_quiet(timezone, now) {
            return None;
        }
        let mut cursor = now
            .with_second(0)
            .and_then(|value| value.with_nanosecond(0))
            .unwrap_or(now);
        let deadline = now + Duration::days(8);
        while cursor < deadline {
            cursor += Duration::minutes(1);
            if !self.is_quiet(timezone, cursor) {
                return Some(cursor);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use chrono_tz::Tz;

    use super::{DaySet, QuietHours, QuietWindow};

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
        let zone = Tz::Europe__Berlin;
        // 23:30 Berlin = 22:30 UTC in winter.
        assert!(quiet.is_quiet(zone, Utc.with_ymd_and_hms(2026, 1, 15, 22, 30, 0).unwrap()));
        // 02:00 Berlin the next morning.
        assert!(quiet.is_quiet(zone, Utc.with_ymd_and_hms(2026, 1, 16, 1, 0, 0).unwrap()));
        // 12:00 Berlin is not quiet.
        assert!(!quiet.is_quiet(zone, Utc.with_ymd_and_hms(2026, 1, 16, 11, 0, 0).unwrap()));
    }

    #[test]
    fn disabled_quiet_hours_never_defer_anything() {
        let mut quiet = nightly();
        quiet.enabled = false;
        assert!(!quiet.is_quiet(
            Tz::Europe__Berlin,
            Utc.with_ymd_and_hms(2026, 1, 15, 22, 30, 0).unwrap()
        ));
    }

    #[test]
    fn the_end_of_the_quiet_period_is_reported_in_utc() {
        let quiet = nightly();
        let zone = Tz::Europe__Berlin;
        let inside = Utc.with_ymd_and_hms(2026, 1, 16, 1, 0, 0).unwrap();
        // 07:00 Berlin = 06:00 UTC in winter.
        assert_eq!(
            quiet.ends_after(zone, inside),
            Some(Utc.with_ymd_and_hms(2026, 1, 16, 6, 0, 0).unwrap())
        );
        let outside = Utc.with_ymd_and_hms(2026, 1, 16, 11, 0, 0).unwrap();
        assert!(quiet.ends_after(zone, outside).is_none());
    }
}
