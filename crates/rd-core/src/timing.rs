//! Shared delay arithmetic: exponential backoff, deterministic jitter and the ceiling on a
//! `Retry-After` some other party chose (audit 1.9.1, INTAKE-12, TR-01, PLUG-04).
//!
//! Before this module the same backoff and the same jitter hash were written out in the
//! subscription scheduler, the notification retry, the capture agent and the transfer
//! engines, and a server's or a plugin's `Retry-After` went into `now + delay` unchecked —
//! where a large enough value panicked the task holding a download slot.

// The ceiling itself is part of the plugin contract and lives in `rd-plugin-types` (RD-1190-08);
// `rd-core` re-exports it.
use rd_plugin_types::MAX_RETRY_AFTER_SECONDS;

/// `Retry-After` and similar waits chosen outside rDownloader, capped at
/// [`MAX_RETRY_AFTER_SECONDS`].
#[must_use]
pub const fn clamp_retry_after(seconds: u64) -> u64 {
    if seconds > MAX_RETRY_AFTER_SECONDS {
        MAX_RETRY_AFTER_SECONDS
    } else {
        seconds
    }
}

/// The spread [`jitter`] gives a retry or a poll, in percent of its delay.
///
/// One figure for the subscription scheduler, the notification retry and the capture agent's
/// reconnect, which each wrote their own `JITTER_PERCENT = 10` before (audit 1.9.1,
/// INTAKE-12): without a spread, everything that failed together retries together, and a
/// service that has just come back up receives its whole backlog in one second.
pub const RETRY_JITTER_PERCENT: u32 = 10;

/// `base · 2^attempt`, capped at `max`, without overflow for any attempt.
#[must_use]
pub fn exponential_backoff(attempt: u32, base: u64, max: u64) -> u64 {
    base.saturating_mul(1_u64 << attempt.min(32)).min(max)
}

/// A deterministic offset within ±`percent` of `delay`.
///
/// Deterministic in `seed` rather than random, so a test can assert on the result and
/// rescheduling the same row twice cannot make the delay wander. Spreading is the purpose:
/// neighbouring seeds land on unrelated offsets, so a backlog of retries does not return in
/// the same second.
#[must_use]
pub fn jitter(delay: i64, percent: u32, seed: u64) -> i64 {
    let span = delay.saturating_mul(i64::from(percent)) / 100;
    if span <= 0 {
        return 0;
    }
    let mixed = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(31)
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let width = u64::try_from(span.saturating_mul(2).saturating_add(1)).unwrap_or(1);
    let magnitude = i64::try_from(mixed % width).unwrap_or(0);
    magnitude - span
}

#[cfg(test)]
mod tests {
    use super::{MAX_RETRY_AFTER_SECONDS, clamp_retry_after, exponential_backoff, jitter};

    #[test]
    fn a_retry_after_is_capped_at_a_day() {
        assert_eq!(clamp_retry_after(30), 30);
        assert_eq!(
            clamp_retry_after(MAX_RETRY_AFTER_SECONDS),
            MAX_RETRY_AFTER_SECONDS
        );
        assert_eq!(clamp_retry_after(u64::MAX), MAX_RETRY_AFTER_SECONDS);
    }

    #[test]
    fn the_backoff_doubles_and_stops_at_its_ceiling_without_overflowing() {
        assert_eq!(exponential_backoff(0, 30, 3600), 30);
        assert_eq!(exponential_backoff(1, 30, 3600), 60);
        assert_eq!(exponential_backoff(5, 30, 3600), 960);
        assert_eq!(exponential_backoff(20, 30, 3600), 3600);
        assert_eq!(exponential_backoff(u32::MAX, u64::MAX, u64::MAX), u64::MAX);
    }

    #[test]
    fn jitter_stays_within_its_band_and_spreads_neighbouring_seeds() {
        let offsets: std::collections::HashSet<i64> =
            (0..50).map(|seed| jitter(1000, 10, seed)).collect();
        assert!(offsets.iter().all(|offset| (-100..=100).contains(offset)));
        assert!(offsets.len() > 10, "neighbouring seeds spread: {offsets:?}");
        assert_eq!(jitter(5, 10, 7), 0, "no band below one unit");
        // Saturates instead of overflowing on an absurd delay.
        let _ = jitter(i64::MAX, 100, 1);
    }
}
