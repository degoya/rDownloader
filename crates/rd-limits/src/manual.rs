//! A profile switched on by hand, in front of the schedule (RD-190-20).
//!
//! The schedule decides which profile is active, except while somebody has picked one
//! themselves — the "slow down for this video call" switch. That choice carries its own end:
//! the schedule's next change as it stood when the switch was made, a time of the person's
//! choosing, or none at all, in which case only switching back ends it. Once the end has passed
//! the schedule is in charge again; nothing here remembers the choice beyond that.

use chrono::{DateTime, Utc};
use rd_core::BandwidthProfileId;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::WeeklySchedule;

/// How a manual switch was told to end. `until` carries the instant; this says where it came
/// from, so the interface can say "until the schedule changes" rather than a bare time.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManualEnd {
    /// At the schedule's next change, fixed when the switch was made. A schedule with no
    /// change ahead leaves `until` empty, so the switch lasts until it is switched back.
    NextSwitch,
    /// At a time the person chose.
    At,
    /// Only when it is switched back.
    Never,
}

/// The profile somebody switched to by hand, and until when.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct ManualProfile {
    /// The profile in force; empty means no limits at all.
    pub profile_id: Option<BandwidthProfileId>,
    pub ends: ManualEnd,
    /// When the schedule takes over again; empty while the switch has no end.
    pub until: Option<DateTime<Utc>>,
    pub switched_at: DateTime<Utc>,
}

impl ManualProfile {
    /// A switch made at `now`. `at` is read only for [`ManualEnd::At`]; the caller has checked
    /// that it lies ahead.
    #[must_use]
    pub fn new(
        profile_id: Option<BandwidthProfileId>,
        ends: ManualEnd,
        at: Option<DateTime<Utc>>,
        schedule: &WeeklySchedule,
        now: DateTime<Utc>,
    ) -> Self {
        let until = match ends {
            ManualEnd::NextSwitch => schedule.next_switch_after(now),
            ManualEnd::At => at,
            ManualEnd::Never => None,
        };
        Self {
            profile_id,
            ends,
            until,
            switched_at: now,
        }
    }

    /// Whether the switch still holds at `now`.
    #[must_use]
    pub fn in_force(&self, now: DateTime<Utc>) -> bool {
        self.until.is_none_or(|until| now < until)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};
    use rd_core::BandwidthProfileId;

    use super::{ManualEnd, ManualProfile};
    use crate::{DaySet, ScheduleWindow, WeeklySchedule};

    fn schedule_with_window(start_minute: u16, end_minute: u16) -> WeeklySchedule {
        WeeklySchedule {
            timezone: chrono_tz::Tz::UTC,
            default_profile_id: None,
            windows: vec![ScheduleWindow {
                id: rd_core::BandwidthWindowId::new(),
                profile_id: BandwidthProfileId::new(),
                days: DaySet::EVERY_DAY,
                start_minute,
                end_minute,
                priority: 0,
                enabled: true,
            }],
        }
    }

    #[test]
    fn until_the_next_switch_is_fixed_when_the_switch_is_made() {
        // 10:00 UTC, inside a window that ends at 12:00.
        let now = Utc
            .with_ymd_and_hms(2026, 10, 2, 10, 0, 0)
            .single()
            .expect("time");
        let schedule = schedule_with_window(8 * 60, 12 * 60);
        let manual = ManualProfile::new(None, ManualEnd::NextSwitch, None, &schedule, now);

        let noon = Utc
            .with_ymd_and_hms(2026, 10, 2, 12, 0, 0)
            .single()
            .expect("time");
        assert_eq!(manual.until, Some(noon));
        assert!(manual.in_force(noon - Duration::seconds(1)));
        assert!(!manual.in_force(noon));
    }

    #[test]
    fn a_schedule_without_a_change_ahead_leaves_the_switch_open() {
        let now = Utc
            .with_ymd_and_hms(2026, 10, 2, 10, 0, 0)
            .single()
            .expect("time");
        let manual = ManualProfile::new(
            Some(BandwidthProfileId::new()),
            ManualEnd::NextSwitch,
            None,
            &WeeklySchedule::default(),
            now,
        );
        assert_eq!(manual.until, None);
        assert!(manual.in_force(now + Duration::days(365)));
    }

    #[test]
    fn a_chosen_time_ends_it_and_never_does_not() {
        let now = Utc
            .with_ymd_and_hms(2026, 10, 2, 10, 0, 0)
            .single()
            .expect("time");
        let later = now + Duration::hours(3);
        let schedule = WeeklySchedule::default();

        let timed = ManualProfile::new(None, ManualEnd::At, Some(later), &schedule, now);
        assert!(timed.in_force(later - Duration::minutes(1)));
        assert!(!timed.in_force(later));

        // A time handed in with `never` is not read.
        let open = ManualProfile::new(None, ManualEnd::Never, Some(later), &schedule, now);
        assert_eq!(open.until, None);
        assert!(open.in_force(later + Duration::days(30)));
    }
}
