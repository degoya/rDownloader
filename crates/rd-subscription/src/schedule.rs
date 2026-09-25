//! When to poll next (RD-080-07).
//!
//! Pure arithmetic over an explicit clock, because the interesting cases — a source that has
//! been failing for a week, a restart in the middle of a backoff, fifty subscriptions added
//! in the same minute — are all about time and none of them are worth reproducing by
//! waiting.
//!
//! Two properties the caller depends on:
//!
//! * **A failing source backs off, and the backoff is bounded.** Hammering a server that is
//!   down does not bring it back, and an unbounded delay would quietly retire the
//!   subscription.
//! * **Due times are spread out.** Subscriptions created together would otherwise poll
//!   together forever, producing a burst every hour instead of a trickle.
//!
//! A cron expression (RD-130-19) is the one exception to the spreading: somebody who wrote
//! `0 6 * * *` means six o'clock, not six o'clock give or take a few minutes, so a scheduled
//! run is never jittered.

use chrono::{DateTime, Duration, TimeZone, Utc};
use rd_core::{MAX_POLL_INTERVAL_SECONDS, MIN_POLL_INTERVAL_SECONDS};

/// Longest cron expression accepted, in bytes. Five fields with lists fit many times over.
pub const MAX_SCHEDULE_LEN: usize = 120;

/// Longest delay a backoff may reach, whatever the failure count.
///
/// Six hours: long enough to stop bothering a server that is genuinely gone, short enough
/// that a source which comes back is noticed the same day.
pub const MAX_BACKOFF_SECONDS: i64 = 6 * 60 * 60;

/// Fraction of the interval used as jitter, in percent.
pub const JITTER_PERCENT: u32 = 10;

/// When to poll next after a successful poll.
///
/// The jitter is deterministic in `seed` rather than random, so a restart does not reshuffle
/// every schedule and a test can assert on the result. Spreading is what it is for, not
/// unpredictability.
#[must_use]
pub fn next_success(now: DateTime<Utc>, interval_seconds: u32, seed: u64) -> DateTime<Utc> {
    let interval = i64::from(clamp_interval(interval_seconds));
    now + Duration::seconds(interval + jitter(interval, seed))
}

/// When to retry after a failed poll.
///
/// Exponential in the number of consecutive failures, starting at the configured interval
/// and capped at [`MAX_BACKOFF_SECONDS`]. One failure is usually a hiccup, so the first
/// retry is not punished harder than the ordinary rhythm.
#[must_use]
pub fn next_failure(
    now: DateTime<Utc>,
    interval_seconds: u32,
    consecutive_failures: u32,
    seed: u64,
) -> DateTime<Utc> {
    let interval = i64::from(clamp_interval(interval_seconds));
    // `saturating_sub(1)` so the first failure waits one plain interval; `min(16)` keeps the
    // shift far away from overflowing before the cap does its work.
    let shift = consecutive_failures.saturating_sub(1).min(16);
    let delay = interval
        .saturating_mul(1_i64 << shift)
        .min(MAX_BACKOFF_SECONDS);
    now + Duration::seconds(delay + jitter(delay, seed))
}

/// Deterministic spread of ±[`JITTER_PERCENT`] around a delay.
fn jitter(delay_seconds: i64, seed: u64) -> i64 {
    let span = delay_seconds * i64::from(JITTER_PERCENT) / 100;
    if span == 0 {
        return 0;
    }
    // A cheap integer hash: the seed is an id, and consecutive ids must not produce
    // consecutive offsets or the spreading does nothing.
    let mixed = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(31)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let magnitude = i64::try_from(mixed % u64::try_from(span * 2 + 1).unwrap_or(1)).unwrap_or(0);
    magnitude - span
}

fn clamp_interval(interval_seconds: u32) -> u32 {
    interval_seconds.clamp(MIN_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS)
}

/// Reads a subscription's cron expression (RD-130-19).
///
/// Five fields -- minute, hour, day of month, month, day of week -- or an alias such as
/// `@daily`, with POSIX weekdays: `0` and `7` are Sunday, `1` is Monday, as in every crontab.
/// Seconds and years are refused rather than accepted, because a sixth field would silently
/// shift the meaning of the other five for somebody who copied a line from a crontab.
///
/// # Errors
///
/// When the expression is empty, too long, or not a cron expression.
pub fn parse_schedule(expression: &str) -> anyhow::Result<croner::Cron> {
    let expression = expression.trim();
    if expression.is_empty() || expression.len() > MAX_SCHEDULE_LEN {
        anyhow::bail!("schedule must be a cron expression of at most {MAX_SCHEDULE_LEN} bytes");
    }
    croner::parser::CronParser::builder()
        .seconds(croner::parser::Seconds::Disallowed)
        .year(croner::parser::Year::Disallowed)
        .build()
        .parse(expression)
        .map_err(|error| anyhow::anyhow!("schedule is not a cron expression: {error}"))
}

/// The first time after `after` that `expression` names, read in `zone`.
///
/// The zone is the service's own in production (`chrono::Local`): `0 6 * * *` is six in the
/// morning where the service runs, including across a change to or from summer time. A test
/// passes a fixed zone instead, so the answer does not depend on the machine running it.
///
/// # Errors
///
/// When the expression does not parse, or names no time at all (`0 0 30 2 *`).
pub fn next_scheduled<Tz: TimeZone>(
    expression: &str,
    after: DateTime<Utc>,
    zone: &Tz,
) -> anyhow::Result<DateTime<Utc>> {
    let cron = parse_schedule(expression)?;
    cron.find_next_occurrence(&after.with_timezone(zone), false)
        .map(|next| next.with_timezone(&Utc))
        .map_err(|error| anyhow::anyhow!("schedule names no future time: {error}"))
}

/// When to retry a scheduled subscription after a failed run.
///
/// The ordinary backoff, but never later than the next scheduled time: a script that fails at
/// six is tried again within the hour rather than tomorrow, and a failure streak cannot push
/// a run past the one the schedule asks for anyway.
#[must_use]
pub fn next_scheduled_failure(
    scheduled: DateTime<Utc>,
    now: DateTime<Utc>,
    interval_seconds: u32,
    consecutive_failures: u32,
    seed: u64,
) -> DateTime<Utc> {
    next_failure(now, interval_seconds, consecutive_failures, seed).min(scheduled)
}

#[cfg(test)]
mod tests {
    use super::{
        JITTER_PERCENT, MAX_BACKOFF_SECONDS, next_failure, next_scheduled, next_scheduled_failure,
        next_success, parse_schedule,
    };
    use chrono::{TimeZone, Utc};
    use rd_core::MIN_POLL_INTERVAL_SECONDS;

    fn now() -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000, 0).single().expect("now")
    }

    #[test]
    fn a_successful_poll_is_rescheduled_about_one_interval_later() {
        let interval = 3_600_i64;
        let span = interval * i64::from(JITTER_PERCENT) / 100;
        for seed in 0..50 {
            let delay = (next_success(now(), 3_600, seed) - now()).num_seconds();
            assert!(
                (interval - span..=interval + span).contains(&delay),
                "seed {seed} gave {delay}"
            );
        }
    }

    #[test]
    fn subscriptions_created_together_do_not_poll_together() {
        // Without spreading, everything added in one minute polls in one burst forever.
        let delays: std::collections::HashSet<i64> = (0..20)
            .map(|seed| (next_success(now(), 3_600, seed) - now()).num_seconds())
            .collect();
        assert!(
            delays.len() > 10,
            "jitter collapsed to {} values",
            delays.len()
        );
    }

    #[test]
    fn the_schedule_is_stable_across_a_restart() {
        // Deterministic in the seed, so reopening the database does not reshuffle every
        // subscription's rhythm.
        assert_eq!(
            next_success(now(), 3_600, 42),
            next_success(now(), 3_600, 42)
        );
    }

    #[test]
    fn a_short_interval_is_clamped_before_it_is_used() {
        let delay = (next_success(now(), 1, 7) - now()).num_seconds();
        let minimum = i64::from(MIN_POLL_INTERVAL_SECONDS);
        let span = minimum * i64::from(JITTER_PERCENT) / 100;
        assert!(delay >= minimum - span, "{delay}");
    }

    #[test]
    fn the_first_failure_waits_one_ordinary_interval() {
        // One failure is usually a hiccup and does not deserve a punishment.
        let delay = (next_failure(now(), 3_600, 1, 3) - now()).num_seconds();
        let span = 3_600 * i64::from(JITTER_PERCENT) / 100;
        assert!((3_600 - span..=3_600 + span).contains(&delay), "{delay}");
    }

    #[test]
    fn repeated_failures_back_off_and_then_stop_growing() {
        let delays: Vec<i64> = (1..=12)
            .map(|failures| (next_failure(now(), 3_600, failures, 5) - now()).num_seconds())
            .collect();
        // Monotone while it is still doubling.
        assert!(delays[0] < delays[1] && delays[1] < delays[2], "{delays:?}");
        // Bounded, or a source that stays down would quietly retire itself.
        let span = MAX_BACKOFF_SECONDS * i64::from(JITTER_PERCENT) / 100;
        for delay in &delays {
            assert!(*delay <= MAX_BACKOFF_SECONDS + span, "{delay}");
        }
        assert!(delays[11] >= MAX_BACKOFF_SECONDS - span, "{:?}", delays[11]);
    }

    fn at(zone: &chrono::FixedOffset, text: &str) -> chrono::DateTime<Utc> {
        let naive =
            chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").expect("naive time");
        zone.from_local_datetime(&naive)
            .single()
            .expect("local time")
            .with_timezone(&Utc)
    }

    #[test]
    fn a_daily_schedule_runs_at_that_hour_in_the_service_s_zone() {
        // RD-130-19. Six in the morning where the service runs, not six UTC.
        let berlin = chrono::FixedOffset::east_opt(2 * 3_600).expect("zone");
        let before = at(&berlin, "2026-09-25 05:00");
        assert_eq!(
            next_scheduled("0 6 * * *", before, &berlin).expect("next"),
            at(&berlin, "2026-09-25 06:00")
        );
        // Exactly at six the run that is due is the one being made, so the next is tomorrow.
        assert_eq!(
            next_scheduled("0 6 * * *", at(&berlin, "2026-09-25 06:00"), &berlin).expect("next"),
            at(&berlin, "2026-09-26 06:00")
        );
        // A plain UTC reading of the same instant gives another answer, which is the point.
        assert_ne!(
            next_scheduled("0 6 * * *", before, &Utc).expect("next"),
            at(&berlin, "2026-09-25 06:00")
        );
    }

    #[test]
    fn weekdays_are_posix_weekdays() {
        // 2026-09-25 is a Friday. `1-5` is Monday to Friday in every crontab; a parser that
        // counted Sunday as 1 would move this to Saturday.
        let zone = chrono::FixedOffset::east_opt(0).expect("zone");
        assert_eq!(
            next_scheduled("30 7 * * 1-5", at(&zone, "2026-09-25 08:00"), &zone).expect("next"),
            at(&zone, "2026-09-28 07:30")
        );
        assert_eq!(
            next_scheduled("0 9 * * 0", at(&zone, "2026-09-25 08:00"), &zone).expect("next"),
            at(&zone, "2026-09-27 09:00")
        );
        assert_eq!(
            next_scheduled("@daily", at(&zone, "2026-09-25 08:00"), &zone).expect("next"),
            at(&zone, "2026-09-26 00:00")
        );
    }

    #[test]
    fn what_is_not_a_five_field_cron_expression_is_refused() {
        for bad in [
            "",
            "   ",
            "every day at six",
            "0 6 * *",
            // Seconds and years would shift the meaning of the five fields people copy.
            "0 0 6 * * *",
            "0 6 * * * 2027",
            "61 6 * * *",
            "0 25 * * *",
        ] {
            assert!(parse_schedule(bad).is_err(), "{bad:?} was accepted");
        }
        assert!(parse_schedule(&"1,".repeat(100)).is_err());
        // Parses, but never happens: refused when the next time is asked for.
        let zone = chrono::FixedOffset::east_opt(0).expect("zone");
        assert!(next_scheduled("0 0 30 2 *", at(&zone, "2026-09-25 08:00"), &zone).is_err());
    }

    #[test]
    fn a_failed_scheduled_run_retries_but_never_past_the_next_scheduled_time() {
        let scheduled = now() + chrono::Duration::hours(20);
        let retry = next_scheduled_failure(scheduled, now(), 3_600, 1, 9);
        assert!(retry < scheduled && retry > now(), "{retry}");
        // A long streak backs off to six hours, which the next scheduled time then caps.
        let soon = now() + chrono::Duration::minutes(30);
        assert_eq!(next_scheduled_failure(soon, now(), 3_600, 12, 9), soon);
    }

    #[test]
    fn an_absurd_failure_count_does_not_overflow() {
        let delay =
            (next_failure(now(), MIN_POLL_INTERVAL_SECONDS, u32::MAX, 1) - now()).num_seconds();
        let span = MAX_BACKOFF_SECONDS * i64::from(JITTER_PERCENT) / 100;
        assert!(delay <= MAX_BACKOFF_SECONDS + span, "{delay}");
        assert!(delay > 0);
    }
}
