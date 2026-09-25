//! Retry policy for a delivery.

use chrono::{DateTime, Duration, Utc};

/// Attempts before a delivery is given up on.
pub const MAX_ATTEMPTS: u32 = 6;

/// Fraction of the backoff used as jitter, in percent.
///
/// The same name and the same figure as `rd_subscription::schedule::JITTER_PERCENT`, because
/// it prevents the same failure: without spreading, every target queued by one event — and
/// every automation run, which shares this policy — retries in the very same second, so a
/// webhook endpoint that has just come back up is hit by the whole backlog at once. That is
/// the burst that took it down, repeated at 30s, 60s, 120s, until the attempts run out.
pub const JITTER_PERCENT: u32 = 10;

/// Backoff after `attempt` failed attempts, capped so a dead target does not push the next
/// try beyond the point where anyone still cares.
///
/// The un-jittered base; anything that schedules more than one delivery wants
/// [`backoff_with_seed`], which spreads them.
#[must_use]
pub fn backoff(attempt: u32) -> Duration {
    // The shift is clamped only to keep it in range; the hour cap is what actually bounds
    // the wait, from the seventh attempt on.
    let seconds = 30_i64.saturating_mul(1_i64 << attempt.min(16));
    Duration::seconds(seconds.min(3600))
}

/// [`backoff`] spread by ±[`JITTER_PERCENT`] around the base delay.
///
/// The spread is deterministic in `seed` rather than random, so a test can assert on the
/// result and rescheduling the same row twice cannot make the delay wander. Spreading is what
/// it is for, not unpredictability.
#[must_use]
pub fn backoff_with_seed(attempt: u32, seed: u64) -> Duration {
    let base = backoff(attempt).num_seconds();
    Duration::seconds(base + jitter(base, seed))
}

/// Deterministic spread of ±[`JITTER_PERCENT`] around a delay.
fn jitter(delay_seconds: i64, seed: u64) -> i64 {
    let span = delay_seconds * i64::from(JITTER_PERCENT) / 100;
    if span == 0 {
        return 0;
    }
    // A cheap integer hash: neighbouring seeds must not produce neighbouring offsets, or the
    // spreading does nothing. Taken from the subscription scheduler so both behave alike.
    let mixed = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(31)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let magnitude = i64::try_from(mixed % u64::try_from(span * 2 + 1).unwrap_or(1)).unwrap_or(0);
    magnitude - span
}

/// When the next attempt is due, or `None` when the delivery has failed for good.
///
/// The seed is the sub-second part of `now`, because that is exactly what tells apart the
/// deliveries this has to spread: the targets of one event fail within milliseconds of each
/// other and each one reads the clock for itself. The due time is persisted, so nothing has
/// to reproduce it after a restart; a caller holding a stable identity can spread on that
/// instead by calling [`backoff_with_seed`] directly.
#[must_use]
pub fn next_attempt_at(attempt: u32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let seed = u64::from(now.timestamp_subsec_nanos());
    (attempt < MAX_ATTEMPTS).then(|| now + backoff_with_seed(attempt, seed))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{JITTER_PERCENT, MAX_ATTEMPTS, backoff, backoff_with_seed, next_attempt_at};

    #[test]
    fn the_backoff_grows_and_then_stops_growing() {
        assert_eq!(backoff(0).num_seconds(), 30);
        assert_eq!(backoff(1).num_seconds(), 60);
        assert_eq!(backoff(5).num_seconds(), 960);
        assert_eq!(backoff(20).num_seconds(), 3600, "capped at an hour");
    }

    #[test]
    fn the_backoff_is_spread_so_a_backlog_does_not_come_back_in_lockstep() {
        // The defect this guards: every target of one event retried in the same second, so an
        // endpoint that had just recovered received the entire backlog at once.
        let base = backoff(3).num_seconds();
        let span = base * i64::from(JITTER_PERCENT) / 100;
        let delays: std::collections::HashSet<i64> = (0..20)
            .map(|seed| backoff_with_seed(3, seed).num_seconds())
            .collect();
        assert!(
            delays.len() > 10,
            "jitter collapsed to {} values",
            delays.len()
        );
        for delay in &delays {
            assert!((base - span..=base + span).contains(delay), "{delay}");
        }
        assert_eq!(
            backoff_with_seed(3, 42),
            backoff_with_seed(3, 42),
            "deterministic in the seed, or a reschedule would move the due time"
        );
    }

    #[test]
    fn the_last_attempt_schedules_nothing_further() {
        let now = Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap();
        assert!(next_attempt_at(0, now).is_some());
        assert!(next_attempt_at(MAX_ATTEMPTS - 1, now).is_some());
        assert!(next_attempt_at(MAX_ATTEMPTS, now).is_none());
    }
}
