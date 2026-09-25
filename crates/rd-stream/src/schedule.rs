//! Turning a schedule into concrete occurrences (RD-080-08).
//!
//! Pure functions over an explicit clock, because everything interesting about a recording
//! schedule is a date problem: the night the clocks go back has an hour that happens twice,
//! the night they go forward has one that never happens at all, and a weekly show at 20:00
//! is at 20:00 in both.
//!
//! The rule throughout is that a *local* time is what the user meant. Storing an offset
//! instead of a zone would move every schedule by an hour twice a year, and nobody would
//! notice until a recording started at the wrong time.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use rd_core::{
    MAX_ROLL_MINUTES, MAX_WINDOW_MINUTES, MINUTES_PER_DAY, ScheduleError, ScheduleKind,
    StreamSchedule,
};

/// One concrete occurrence, in UTC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Occurrence {
    /// The nominal start. This is the idempotency key: one row per schedule and instant.
    pub starts_at: DateTime<Utc>,
    /// The nominal end, before any post-roll.
    pub ends_at: DateTime<Utc>,
}

impl Occurrence {
    /// The interval the monitor actually watches, widened by the lead and trail.
    #[must_use]
    pub fn watch_window(&self, schedule: &StreamSchedule) -> (DateTime<Utc>, DateTime<Utc>) {
        (
            self.starts_at - Duration::minutes(i64::from(schedule.lead_minutes)),
            self.ends_at + Duration::minutes(i64::from(schedule.trail_minutes)),
        )
    }
}

/// Validates a schedule's fields, returning the resolved time zone.
pub fn validate(schedule: &StreamSchedule) -> Result<Tz, ScheduleError> {
    let zone: Tz = schedule
        .timezone
        .parse()
        .map_err(|_| ScheduleError::UnknownTimezone)?;
    if schedule.window_minutes == 0 || schedule.window_minutes > MAX_WINDOW_MINUTES {
        return Err(ScheduleError::WindowOutOfRange);
    }
    if schedule.lead_minutes > MAX_ROLL_MINUTES || schedule.trail_minutes > MAX_ROLL_MINUTES {
        return Err(ScheduleError::RollOutOfRange);
    }
    if let ScheduleKind::Weekly { days, start_minute } = &schedule.kind {
        if days.is_empty() {
            return Err(ScheduleError::NoDays);
        }
        if days.iter().any(|day| !(1..=7).contains(day)) {
            return Err(ScheduleError::InvalidDay);
        }
        if *start_minute >= MINUTES_PER_DAY {
            return Err(ScheduleError::StartOutOfRange);
        }
    }
    Ok(zone)
}

/// Every occurrence whose window overlaps `[from, to]`.
///
/// Overlap rather than containment, so a run that is already under way when the service
/// starts is still found — otherwise a restart in the middle of a broadcast would lose it.
pub fn occurrences(
    schedule: &StreamSchedule,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<Occurrence>, ScheduleError> {
    let zone = validate(schedule)?;
    let window = Duration::minutes(i64::from(schedule.window_minutes));

    match &schedule.kind {
        ScheduleKind::Once { start } => {
            let occurrence = Occurrence {
                starts_at: *start,
                ends_at: *start + window,
            };
            Ok(
                if occurrence.ends_at >= from && occurrence.starts_at <= to {
                    vec![occurrence]
                } else {
                    Vec::new()
                },
            )
        }
        ScheduleKind::Weekly { days, start_minute } => {
            let mut found = Vec::new();
            // Start a day early: an occurrence that began yesterday can still be running.
            let first = (from - window - Duration::days(1))
                .with_timezone(&zone)
                .date_naive();
            let last = to.with_timezone(&zone).date_naive();
            let mut date = first;
            while date <= last {
                if days.contains(&day_number(date))
                    && let Some(starts_at) = local_start(zone, date, *start_minute)
                {
                    let occurrence = Occurrence {
                        starts_at,
                        ends_at: starts_at + window,
                    };
                    if occurrence.ends_at >= from && occurrence.starts_at <= to {
                        found.push(occurrence);
                    }
                }
                let Some(next) = date.succ_opt() else { break };
                date = next;
            }
            found.sort_by_key(|occurrence| occurrence.starts_at);
            found.dedup_by_key(|occurrence| occurrence.starts_at);
            Ok(found)
        }
    }
}

/// ISO weekday, `1` = Monday through `7` = Sunday.
fn day_number(date: NaiveDate) -> u8 {
    u8::try_from(date.weekday().number_from_monday()).unwrap_or(1)
}

/// Resolves a local wall-clock time to an instant, handling both daylight-saving edges.
///
/// * **Ambiguous** (the clocks went back, so the hour happens twice): the *first* of the two
///   is used. Recording the earlier one and stopping is better than recording an hour late,
///   and picking deterministically is what stops the same window producing two runs.
/// * **Nonexistent** (the clocks went forward, so the hour never happens): `None`. A window
///   at a time that does not exist on that date genuinely has no occurrence, and inventing
///   one by shifting it would start a recording at a time the user never asked for.
fn local_start(zone: Tz, date: NaiveDate, start_minute: u32) -> Option<DateTime<Utc>> {
    let time = NaiveTime::from_num_seconds_from_midnight_opt(start_minute * 60, 0)?;
    match zone.from_local_datetime(&date.and_time(time)) {
        LocalResult::Single(local) => Some(local.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

/// Whether `now` falls inside an occurrence's watch window.
#[must_use]
pub fn is_watching(schedule: &StreamSchedule, occurrence: &Occurrence, now: DateTime<Utc>) -> bool {
    let (start, end) = occurrence.watch_window(schedule);
    now >= start && now <= end
}

#[cfg(test)]
mod tests {
    use super::{Occurrence, is_watching, occurrences, validate};
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use rd_core::{ScheduleError, ScheduleKind, StreamSchedule};

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn weekly(days: Vec<u8>, start_minute: u32, timezone: &str) -> StreamSchedule {
        StreamSchedule {
            id: rd_core::StreamScheduleId::new(),
            channel_id: rd_core::StreamChannelId::new(),
            name: "Show".to_owned(),
            enabled: true,
            kind: ScheduleKind::Weekly { days, start_minute },
            timezone: timezone.to_owned(),
            window_minutes: 120,
            lead_minutes: 5,
            trail_minutes: 10,
            replay_from_start: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn a_weekly_window_keeps_its_local_time_across_daylight_saving() {
        // The property the whole design exists for: 20:00 Berlin is 20:00 Berlin in winter
        // and in summer, even though that is 19:00 UTC and 18:00 UTC respectively. Storing
        // an offset instead of a zone would move the show by an hour twice a year.
        let schedule = weekly(vec![3], 20 * 60, "Europe/Berlin");

        let winter = occurrences(
            &schedule,
            at("2026-01-06T00:00:00Z"),
            at("2026-01-08T23:00:00Z"),
        )
        .expect("winter");
        assert_eq!(winter.len(), 1);
        assert_eq!(winter[0].starts_at, at("2026-01-07T19:00:00Z"));

        let summer = occurrences(
            &schedule,
            at("2026-07-07T00:00:00Z"),
            at("2026-07-09T23:00:00Z"),
        )
        .expect("summer");
        assert_eq!(summer.len(), 1);
        assert_eq!(summer[0].starts_at, at("2026-07-08T18:00:00Z"));
    }

    #[test]
    fn the_hour_that_happens_twice_produces_one_occurrence() {
        // Clocks go back in Europe on 2026-10-25 at 03:00 local, so 02:30 exists twice.
        // Two rows for one broadcast is exactly the duplicate the idempotency key prevents,
        // and it has to be prevented here too or two different instants would be planned.
        let schedule = weekly(vec![7], 2 * 60 + 30, "Europe/Berlin");
        let found = occurrences(
            &schedule,
            at("2026-10-24T00:00:00Z"),
            at("2026-10-26T00:00:00Z"),
        )
        .expect("occurrences");
        assert_eq!(found.len(), 1, "{found:?}");
        // The earlier of the two, deterministically: 02:30 CEST is 00:30 UTC.
        assert_eq!(found[0].starts_at, at("2026-10-25T00:30:00Z"));
    }

    #[test]
    fn the_hour_that_never_happens_produces_no_occurrence() {
        // Clocks go forward on 2026-03-29 at 02:00 local, so 02:30 does not exist that day.
        // Shifting it would start a recording at a time nobody asked for.
        let schedule = weekly(vec![7], 2 * 60 + 30, "Europe/Berlin");
        let found = occurrences(
            &schedule,
            at("2026-03-28T00:00:00Z"),
            at("2026-03-30T00:00:00Z"),
        )
        .expect("occurrences");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn planning_the_same_range_twice_yields_the_same_instants() {
        // What makes the plan restart-safe: the second pass has to agree with the first, or
        // the idempotency key would never match and every restart would duplicate.
        let schedule = weekly(vec![1, 3, 5], 18 * 60, "America/New_York");
        let range = (at("2026-02-01T00:00:00Z"), at("2026-02-28T00:00:00Z"));
        let first = occurrences(&schedule, range.0, range.1).expect("first");
        let second = occurrences(&schedule, range.0, range.1).expect("second");
        assert_eq!(first, second);
        assert!(first.len() > 8, "{}", first.len());
    }

    #[test]
    fn an_occurrence_already_under_way_is_still_found() {
        // A restart in the middle of a broadcast must not lose the run.
        let schedule = weekly(vec![3], 20 * 60, "Europe/Berlin");
        let mid_show = at("2026-01-07T19:30:00Z");
        let found =
            occurrences(&schedule, mid_show, mid_show + Duration::minutes(1)).expect("occurrences");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].starts_at, at("2026-01-07T19:00:00Z"));
    }

    #[test]
    fn the_watch_window_is_widened_by_the_lead_and_trail() {
        let schedule = weekly(vec![3], 20 * 60, "Europe/Berlin");
        let occurrence = Occurrence {
            starts_at: at("2026-01-07T19:00:00Z"),
            ends_at: at("2026-01-07T21:00:00Z"),
        };
        let (start, end) = occurrence.watch_window(&schedule);
        assert_eq!(start, at("2026-01-07T18:55:00Z"));
        assert_eq!(end, at("2026-01-07T21:10:00Z"));

        assert!(!is_watching(
            &schedule,
            &occurrence,
            at("2026-01-07T18:54:00Z")
        ));
        // Inside the pre-roll: a broadcast that starts early is still caught.
        assert!(is_watching(
            &schedule,
            &occurrence,
            at("2026-01-07T18:56:00Z")
        ));
        assert!(is_watching(
            &schedule,
            &occurrence,
            at("2026-01-07T21:09:00Z")
        ));
        assert!(!is_watching(
            &schedule,
            &occurrence,
            at("2026-01-07T21:11:00Z")
        ));
    }

    #[test]
    fn a_one_off_fires_at_its_absolute_instant() {
        let mut schedule = weekly(vec![1], 0, "Europe/Berlin");
        schedule.kind = ScheduleKind::Once {
            start: at("2026-05-01T12:00:00Z"),
        };
        let found = occurrences(
            &schedule,
            at("2026-05-01T00:00:00Z"),
            at("2026-05-02T00:00:00Z"),
        )
        .expect("occurrences");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].starts_at, at("2026-05-01T12:00:00Z"));
        assert_eq!(found[0].ends_at, at("2026-05-01T14:00:00Z"));

        // And not outside its own day.
        assert!(
            occurrences(
                &schedule,
                at("2026-06-01T00:00:00Z"),
                at("2026-06-02T00:00:00Z")
            )
            .expect("occurrences")
            .is_empty()
        );
    }

    #[test]
    fn a_schedule_that_could_never_fire_is_refused() {
        let mut schedule = weekly(vec![], 0, "Europe/Berlin");
        assert_eq!(validate(&schedule), Err(ScheduleError::NoDays));

        schedule = weekly(vec![8], 0, "Europe/Berlin");
        assert_eq!(validate(&schedule), Err(ScheduleError::InvalidDay));

        schedule = weekly(vec![1], 24 * 60, "Europe/Berlin");
        assert_eq!(validate(&schedule), Err(ScheduleError::StartOutOfRange));

        schedule = weekly(vec![1], 0, "Not/AZone");
        assert_eq!(validate(&schedule), Err(ScheduleError::UnknownTimezone));

        // An offset is not a zone: it does not survive daylight saving, which is the whole
        // reason this field exists.
        schedule = weekly(vec![1], 0, "+01:00");
        assert_eq!(validate(&schedule), Err(ScheduleError::UnknownTimezone));
    }

    #[test]
    fn a_window_or_roll_out_of_range_is_refused() {
        let mut schedule = weekly(vec![1], 0, "UTC");
        schedule.window_minutes = 0;
        assert_eq!(validate(&schedule), Err(ScheduleError::WindowOutOfRange));

        schedule = weekly(vec![1], 0, "UTC");
        schedule.window_minutes = 24 * 60 + 1;
        assert_eq!(validate(&schedule), Err(ScheduleError::WindowOutOfRange));

        schedule = weekly(vec![1], 0, "UTC");
        schedule.lead_minutes = 121;
        assert_eq!(validate(&schedule), Err(ScheduleError::RollOutOfRange));
    }

    #[test]
    fn a_southern_hemisphere_zone_shifts_the_other_way() {
        // Sydney's daylight saving runs opposite to Europe's, which is a cheap way to catch
        // a sign error that Berlin alone would hide.
        let schedule = weekly(vec![3], 20 * 60, "Australia/Sydney");
        let january = occurrences(
            &schedule,
            at("2026-01-06T00:00:00Z"),
            at("2026-01-08T23:00:00Z"),
        )
        .expect("january");
        // AEDT is UTC+11 in January.
        assert_eq!(january[0].starts_at, at("2026-01-07T09:00:00Z"));

        let july = occurrences(
            &schedule,
            at("2026-07-07T00:00:00Z"),
            at("2026-07-09T23:00:00Z"),
        )
        .expect("july");
        // AEST is UTC+10 in July.
        assert_eq!(july[0].starts_at, at("2026-07-08T10:00:00Z"));
    }

    #[test]
    fn utc_is_accepted_and_needs_no_conversion() {
        let schedule = weekly(vec![3], 20 * 60, "UTC");
        let found = occurrences(
            &schedule,
            at("2026-01-06T00:00:00Z"),
            at("2026-01-08T23:00:00Z"),
        )
        .expect("occurrences");
        assert_eq!(found[0].starts_at, at("2026-01-07T20:00:00Z"));
        let _ = Utc.timestamp_opt(0, 0);
    }
}
