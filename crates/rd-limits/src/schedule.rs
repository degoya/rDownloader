//! Weekly profile schedule.
//!
//! Every evaluation starts from a UTC instant and asks the timezone what the local time is
//! there. Nothing adds twenty-four hours to a local time, which is exactly the arithmetic
//! that loses or duplicates an hour across a DST change.

use jiff::civil::Weekday;
use jiff::tz::TimeZone;
use rd_core::{BandwidthProfileId, BandwidthWindowId, IanaZone, SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Minutes in a day; a window end of `MINUTES_PER_DAY` means "up to midnight".
pub const MINUTES_PER_DAY: u16 = 24 * 60;

/// How far ahead [`WeeklySchedule::next_switch_after`] looks before giving up.
const LOOKAHEAD_DAYS: i64 = 8;

/// Days a window applies to, as a Monday-first bitmask (bit 0 = Monday).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(transparent)]
pub struct DaySet(pub u8);

impl DaySet {
    pub const EVERY_DAY: Self = Self(0b0111_1111);

    #[must_use]
    pub fn contains(self, weekday: Weekday) -> bool {
        self.0 & (1 << weekday.to_monday_zero_offset()) != 0
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 & Self::EVERY_DAY.0 == 0
    }
}

/// One time window that activates a profile.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ScheduleWindow {
    pub id: BandwidthWindowId,
    pub profile_id: BandwidthProfileId,
    pub days: DaySet,
    /// Minutes since local midnight.
    pub start_minute: u16,
    /// Exclusive end; a value below `start_minute` wraps past midnight into the next day.
    pub end_minute: u16,
    /// Higher wins where windows overlap; ties go to the later start.
    pub priority: i32,
    pub enabled: bool,
}

impl ScheduleWindow {
    /// Whether the window covers a local weekday/minute pair.
    ///
    /// A wrapping window belongs to the day it *starts* on, so "Fri 22:00–02:00" is still
    /// active at Saturday 01:00.
    #[must_use]
    fn covers(&self, weekday: Weekday, minute: u16) -> bool {
        self.enabled
            && covers_local(
                self.days,
                self.start_minute,
                self.end_minute,
                weekday,
                minute,
            )
    }
}

/// Whether a weekly window covers a local weekday/minute pair.
///
/// A wrapping window belongs to the day it *starts* on, so "Fri 22:00–02:00" is still
/// active at Saturday 01:00. Shared with the quiet hours, which use the same rule.
#[must_use]
pub fn covers_local(
    days: DaySet,
    start_minute: u16,
    end_minute: u16,
    weekday: Weekday,
    minute: u16,
) -> bool {
    if days.is_empty() {
        return false;
    }
    if start_minute < end_minute {
        return days.contains(weekday) && (start_minute..end_minute).contains(&minute);
    }
    (days.contains(weekday) && minute >= start_minute)
        || (days.contains(weekday.previous()) && minute < end_minute)
}

/// The local weekday and minute of a UTC instant in `timezone`.
#[must_use]
pub fn local_position(timezone: &TimeZone, now: Timestamp) -> (Weekday, u16) {
    let local = timezone.to_datetime(now.as_jiff());
    let minute =
        u16::try_from(i32::from(local.hour()) * 60 + i32::from(local.minute())).unwrap_or(0);
    (local.weekday(), minute)
}

/// `now` with its seconds and fraction dropped: the minute boundary at or before it, the step
/// the schedule walks take.
pub(crate) fn floor_to_minute(now: Timestamp) -> Timestamp {
    Timestamp::from_unix_seconds(now.unix_seconds().div_euclid(60) * 60).unwrap_or(now)
}

/// The whole schedule: a timezone, the windows and the profile that applies outside them.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct WeeklySchedule {
    /// IANA timezone name; the windows are local times in it.
    #[schema(value_type = String, example = "Europe/Berlin")]
    pub timezone: IanaZone,
    /// Profile used whenever no window applies; `None` means unlimited.
    pub default_profile_id: Option<BandwidthProfileId>,
    pub windows: Vec<ScheduleWindow>,
}

impl Default for WeeklySchedule {
    fn default() -> Self {
        Self {
            timezone: IanaZone::utc(),
            default_profile_id: None,
            windows: Vec::new(),
        }
    }
}

impl WeeklySchedule {
    /// The profile active at `now`.
    #[must_use]
    pub fn active_at(&self, now: Timestamp) -> Option<BandwidthProfileId> {
        let (weekday, minute) = local_position(self.timezone.as_jiff(), now);
        self.windows
            .iter()
            .filter(|window| window.covers(weekday, minute))
            // Overlaps resolve deterministically: priority first, then the later start,
            // then the id — so the same schedule always picks the same window.
            .max_by(|left, right| {
                left.priority
                    .cmp(&right.priority)
                    .then(left.start_minute.cmp(&right.start_minute))
                    .then(left.id.to_string().cmp(&right.id.to_string()))
            })
            .map(|window| window.profile_id)
            .or(self.default_profile_id)
    }

    /// The next instant at which [`Self::active_at`] changes its answer.
    ///
    /// Found by walking forward in UTC and converting to local time at each step, so a DST
    /// jump neither skips a switch nor invents a second one.
    #[must_use]
    pub fn next_switch_after(&self, now: Timestamp) -> Option<Timestamp> {
        let current = self.active_at(now);
        // Aligning to the minute keeps the walk on the boundaries windows are defined on.
        let mut cursor = floor_to_minute(now);
        let deadline = now + SignedDuration::from_hours(24 * LOOKAHEAD_DAYS);
        while cursor < deadline {
            cursor += SignedDuration::from_mins(1);
            if self.active_at(cursor) != current {
                return Some(cursor);
            }
        }
        None
    }

    /// Day key of a UTC instant in the schedule's timezone, e.g. `2026-09-03`.
    #[must_use]
    pub fn day_key(&self, now: Timestamp) -> String {
        rd_core::Date::from_jiff(self.timezone.as_jiff().to_datetime(now.as_jiff()).date())
            .to_string()
    }

    /// Month key of a UTC instant in the schedule's timezone, e.g. `2026-09`.
    #[must_use]
    pub fn month_key(&self, now: Timestamp) -> String {
        self.timezone
            .as_jiff()
            .to_datetime(now.as_jiff())
            .strftime("%Y-%m")
            .to_string()
    }
}

/// The zone every schedule falls back to, matching the configured default.
///
/// One place, so a stored value that cannot be parsed lands in the same zone a fresh install
/// uses rather than silently in UTC an hour away.
#[must_use]
pub fn default_timezone() -> IanaZone {
    IanaZone::parse(rd_core::DEFAULT_BANDWIDTH_TIMEZONE).unwrap_or_else(|_| IanaZone::utc())
}

/// The IANA zone called exactly `value` (case-sensitive, as stored).
///
/// # Errors
/// When no zone has that name.
pub fn parse_timezone(value: &str) -> anyhow::Result<IanaZone> {
    IanaZone::parse(value).map_err(|_| anyhow::anyhow!("unknown timezone `{value}`"))
}

#[cfg(test)]
mod tests {
    use rd_core::{BandwidthProfileId, BandwidthWindowId, IanaZone, Timestamp};

    use super::{DaySet, ScheduleWindow, WeeklySchedule};

    fn window(profile: BandwidthProfileId, start: u16, end: u16, priority: i32) -> ScheduleWindow {
        ScheduleWindow {
            id: BandwidthWindowId::new(),
            profile_id: profile,
            days: DaySet::EVERY_DAY,
            start_minute: start,
            end_minute: end,
            priority,
            enabled: true,
        }
    }

    fn berlin(windows: Vec<ScheduleWindow>, default: Option<BandwidthProfileId>) -> WeeklySchedule {
        WeeklySchedule {
            timezone: IanaZone::parse("Europe/Berlin").expect("zone"),
            default_profile_id: default,
            windows,
        }
    }

    #[test]
    fn a_window_activates_its_profile_in_local_time() {
        let night = BandwidthProfileId::new();
        let day = BandwidthProfileId::new();
        // 22:00–06:00 local, so the window wraps past midnight.
        let schedule = berlin(vec![window(night, 22 * 60, 6 * 60, 0)], Some(day));
        // 2026-01-15 23:30 Berlin = 22:30 UTC (CET, UTC+1).
        let inside = Timestamp::from_ymd_hms(2026, 1, 15, 22, 30, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(inside), Some(night));
        // 2026-01-16 01:30 Berlin = 00:30 UTC — still the wrapping window.
        let after_midnight =
            Timestamp::from_ymd_hms(2026, 1, 16, 0, 30, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(after_midnight), Some(night));
        // 2026-01-16 12:00 Berlin = 11:00 UTC — outside, so the default applies.
        let outside = Timestamp::from_ymd_hms(2026, 1, 16, 11, 0, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(outside), Some(day));
    }

    #[test]
    fn overlapping_windows_resolve_by_priority_then_later_start() {
        let broad = BandwidthProfileId::new();
        let narrow = BandwidthProfileId::new();
        let schedule = berlin(
            vec![
                window(broad, 8 * 60, 20 * 60, 0),
                window(narrow, 12 * 60, 14 * 60, 5),
            ],
            None,
        );
        // 13:00 Berlin = 12:00 UTC in winter.
        let overlap = Timestamp::from_ymd_hms(2026, 1, 15, 12, 0, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(overlap), Some(narrow));
        let only_broad = Timestamp::from_ymd_hms(2026, 1, 15, 9, 0, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(only_broad), Some(broad));
    }

    #[test]
    fn a_disabled_window_does_not_apply() {
        let profile = BandwidthProfileId::new();
        let default = BandwidthProfileId::new();
        let mut disabled = window(profile, 0, super::MINUTES_PER_DAY, 0);
        disabled.enabled = false;
        let schedule = berlin(vec![disabled], Some(default));
        let now = Timestamp::from_ymd_hms(2026, 1, 15, 12, 0, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(now), Some(default));
    }

    #[test]
    fn the_spring_forward_hour_neither_loses_nor_duplicates_a_switch() {
        let night = BandwidthProfileId::new();
        let day = BandwidthProfileId::new();
        // Germany skips 02:00–03:00 local on 2026-03-29; a 02:30 boundary never happens.
        let schedule = berlin(vec![window(night, 0, 3 * 60, 0)], Some(day));
        // 00:30 UTC = 01:30 CET, still inside the night window.
        let before = Timestamp::from_ymd_hms(2026, 3, 29, 0, 30, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(before), Some(night));
        // 01:00 UTC = 03:00 CEST — the clock jumped straight past the window's end.
        let after = Timestamp::from_ymd_hms(2026, 3, 29, 1, 0, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(after), Some(day));
        let switch = schedule
            .next_switch_after(before)
            .expect("a switch follows");
        assert_eq!(
            switch,
            Timestamp::from_ymd_hms(2026, 3, 29, 1, 0, 0).expect("a valid instant")
        );
    }

    #[test]
    fn the_autumn_hour_that_happens_twice_produces_one_period_each_time() {
        let night = BandwidthProfileId::new();
        let day = BandwidthProfileId::new();
        // Germany repeats 02:00–03:00 local on 2026-10-25 (CEST then CET).
        let schedule = berlin(vec![window(night, 2 * 60 + 30, 6 * 60, 0)], Some(day));
        // 00:00 UTC = 02:00 CEST — before the window starts.
        let first_pass = Timestamp::from_ymd_hms(2026, 10, 25, 0, 0, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(first_pass), Some(day));
        // 00:45 UTC = 02:45 CEST — inside the window on its first pass.
        let inside_first =
            Timestamp::from_ymd_hms(2026, 10, 25, 0, 45, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(inside_first), Some(night));
        // 01:15 UTC = 02:15 CET — the repeated hour, before the window starts again.
        let repeated = Timestamp::from_ymd_hms(2026, 10, 25, 1, 15, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(repeated), Some(day));
        // 01:45 UTC = 02:45 CET — inside again on the second pass.
        let inside_second =
            Timestamp::from_ymd_hms(2026, 10, 25, 1, 45, 0).expect("a valid instant");
        assert_eq!(schedule.active_at(inside_second), Some(night));
    }

    #[test]
    fn period_keys_follow_the_schedule_timezone_not_utc() {
        let schedule = berlin(Vec::new(), None);
        // 2026-08-31 23:30 UTC is already 2026-09-01 in Berlin (CEST, UTC+2).
        let instant = Timestamp::from_ymd_hms(2026, 8, 31, 23, 30, 0).expect("a valid instant");
        assert_eq!(schedule.day_key(instant), "2026-09-01");
        assert_eq!(schedule.month_key(instant), "2026-09");
    }

    #[test]
    fn a_schedule_without_windows_never_switches() {
        let schedule = berlin(Vec::new(), Some(BandwidthProfileId::new()));
        let now = Timestamp::from_ymd_hms(2026, 1, 15, 12, 0, 0).expect("a valid instant");
        assert!(schedule.next_switch_after(now).is_none());
    }
}
