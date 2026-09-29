//! When a scheduled backup is due (RD-160-01).
//!
//! The cron arithmetic is the subscriptions' (`rd_subscription::next_scheduled`, RD-130-19):
//! five POSIX fields, no seconds, no years. What this adds is the zone — an IANA name the
//! person chose instead of the service's local zone — and the decision the service's loop makes
//! at every tick, as a pure function of the stored due time and the clock, so a restart and a
//! change to or from summer time can be tested without waiting for either.
//!
//! Two properties the loop relies on:
//!
//! * **The due time is persisted before the run starts.** A restart reads it back; it neither
//!   repeats a run that already started nor skips one that was due while the service was down.
//! * **Missed runs are caught up once.** After a week offline the next start runs one backup,
//!   not seven: the next due time is computed from *now*, not from the missed one.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// The zone an IANA name stands for, if it is one.
#[must_use]
pub fn parse_timezone(name: &str) -> Option<Tz> {
    name.trim().parse().ok()
}

/// The first time after `after` that `schedule` names, read in `timezone`.
///
/// Over a change to summer time a fixed time that does not exist that night (02:30 in
/// Europe/Berlin on the last Sunday of March) runs at the first minute after the gap; over the
/// change back a time that exists twice runs once, at its first occurrence.
///
/// # Errors
///
/// When the zone is unknown, the expression does not parse, or it names no time at all.
pub fn next_run(
    schedule: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> anyhow::Result<DateTime<Utc>> {
    let zone =
        parse_timezone(timezone).ok_or_else(|| anyhow::anyhow!("unknown time zone {timezone}"))?;
    rd_subscription::next_scheduled(schedule, after, &zone)
}

/// What the loop does at one tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Due {
    /// The schedule is off.
    Idle,
    /// Switched on but never timed: store this due time, run nothing yet.
    Arm(DateTime<Utc>),
    /// Not yet.
    Wait,
    /// Due: store `next` first, then run.
    Run { next: DateTime<Utc> },
}

/// The decision at `now`, from what is stored.
///
/// # Errors
///
/// When the stored schedule or zone no longer yields a time.
pub fn decide(
    enabled: bool,
    schedule: &str,
    timezone: &str,
    next_run_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> anyhow::Result<Due> {
    if !enabled {
        return Ok(Due::Idle);
    }
    match next_run_at {
        None => Ok(Due::Arm(next_run(schedule, timezone, now)?)),
        Some(due) if due > now => Ok(Due::Wait),
        Some(_) => Ok(Due::Run {
            next: next_run(schedule, timezone, now)?,
        }),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
    use chrono_tz::Tz;

    use super::{Due, decide, next_run};

    const BERLIN: &str = "Europe/Berlin";

    fn utc(text: &str) -> DateTime<Utc> {
        let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").expect("time");
        Utc.from_utc_datetime(&naive)
    }

    fn berlin(text: &str) -> DateTime<Utc> {
        let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").expect("time");
        let zone: Tz = BERLIN.parse().expect("zone");
        zone.from_local_datetime(&naive)
            .earliest()
            .expect("local time")
            .with_timezone(&Utc)
    }

    #[test]
    fn a_daily_backup_keeps_its_wall_clock_hour_across_both_changes() {
        // 2026-03-29 and 2026-10-25 are Berlin's changes. 03:00 exists on both nights.
        assert_eq!(
            next_run("0 3 * * *", BERLIN, berlin("2026-03-28 03:00")).expect("next"),
            utc("2026-03-29 01:00"),
            "23 hours later in UTC, still three o'clock on the wall"
        );
        assert_eq!(
            next_run("0 3 * * *", BERLIN, berlin("2026-10-24 03:00")).expect("next"),
            utc("2026-10-25 02:00"),
            "25 hours later in UTC"
        );
    }

    #[test]
    fn a_time_the_spring_change_skips_runs_right_after_the_gap_and_one_it_repeats_runs_once() {
        // 02:30 does not exist on 2026-03-29 in Berlin: the run moves to 03:00, the same day.
        assert_eq!(
            next_run("30 2 * * *", BERLIN, berlin("2026-03-28 02:30")).expect("next"),
            utc("2026-03-29 01:00")
        );
        // 02:30 exists twice on 2026-10-25: once at 00:30 UTC (summer), once at 01:30 UTC.
        let first = next_run("30 2 * * *", BERLIN, berlin("2026-10-24 02:30")).expect("next");
        assert_eq!(first, utc("2026-10-25 00:30"));
        let then = next_run("30 2 * * *", BERLIN, first).expect("next");
        assert_eq!(
            then,
            utc("2026-10-26 01:30"),
            "not a second run an hour later"
        );
    }

    #[test]
    fn a_restart_reads_the_stored_due_time_back_and_neither_repeats_nor_skips() {
        let stored = berlin("2026-09-29 03:00");
        // Restarted before it was due: nothing to do, and the stored time is kept.
        assert_eq!(
            decide(
                true,
                "0 3 * * *",
                BERLIN,
                Some(stored),
                berlin("2026-09-29 02:59")
            )
            .expect("decide"),
            Due::Wait
        );
        // Restarted after it was due (the service was down at three): one run now.
        let now = berlin("2026-09-29 07:15");
        let Due::Run { next } =
            decide(true, "0 3 * * *", BERLIN, Some(stored), now).expect("decide")
        else {
            panic!("a missed run is caught up");
        };
        assert_eq!(next, berlin("2026-09-30 03:00"));
        // The loop stores `next` before running, so the tick after the run waits.
        assert_eq!(
            decide(true, "0 3 * * *", BERLIN, Some(next), now).expect("decide"),
            Due::Wait
        );
    }

    #[test]
    fn a_week_offline_is_caught_up_with_one_run_not_seven() {
        let stored = berlin("2026-09-01 03:00");
        let now = berlin("2026-09-08 12:00");
        let Due::Run { next } =
            decide(true, "0 3 * * *", BERLIN, Some(stored), now).expect("decide")
        else {
            panic!("due");
        };
        assert!(next > now);
        assert_eq!(next, berlin("2026-09-09 03:00"));
    }

    #[test]
    fn switched_on_it_is_timed_first_and_switched_off_it_does_nothing() {
        let now = berlin("2026-09-28 12:00");
        assert_eq!(
            decide(true, "0 3 * * *", BERLIN, None, now).expect("decide"),
            Due::Arm(berlin("2026-09-29 03:00"))
        );
        assert_eq!(
            decide(false, "0 3 * * *", BERLIN, Some(now), now).expect("decide"),
            Due::Idle
        );
    }

    #[test]
    fn an_unknown_zone_or_expression_is_an_error_not_a_guess() {
        let now = utc("2026-09-28 12:00");
        assert!(next_run("0 3 * * *", "Mars/Olympus", now).is_err());
        assert!(next_run("every night", BERLIN, now).is_err());
        assert!(next_run("0 0 30 2 *", BERLIN, now).is_err());
    }
}
