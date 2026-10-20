use std::time::Duration;

use chrono::{DateTime, Utc};
use rd_core::{Failure, FailureKind};

/// Upper bound accepted for the configurable retry count.
pub const MAX_CONFIGURABLE_RETRIES: u32 = 100;

/// Hold-off applied when a hoster reports an IP limit without naming a duration. Free
/// download limits are counted in tens of minutes, so the exponential backoff (five
/// minutes at most) would retry far too eagerly.
const DEFAULT_IP_BLOCK: Duration = Duration::from_secs(15 * 60);

/// The longest wait the exponential backoff alone reaches.
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);

/// Failed attempts in a row on the seconds-long backoff (RD-1250-07); from the sixth on a
/// download leaves the queue place to the files after it rather than taking it again every few
/// seconds.
pub(crate) const FAST_RETRIES: u32 = 5;

/// The first wait of the middle stage after the fast retries; each further one doubles it
/// (RD-1340-01). A hoster's 503 for one file lasted from forty seconds to a few minutes: the
/// fast retries were spent in half a minute and the file then sat out the long wait.
const MIDDLE_WAIT: Duration = Duration::from_secs(60);

/// Default attempts of the middle stage: one, two and four minutes.
pub const DEFAULT_RETRY_MIDDLE_ATTEMPTS: u32 = 3;
/// Upper bound accepted for the middle stage's attempts.
pub const MAX_RETRY_MIDDLE_ATTEMPTS: u32 = 10;

/// Default long wait after the middle stage, in minutes (RD-1250-07): long enough for the files
/// after it to run, short enough for a server that recovers to be asked again the same hour.
pub const DEFAULT_RETRY_LONG_WAIT_MINUTES: u32 = 30;
/// Bounds accepted for the long wait, in minutes.
pub const MIN_RETRY_LONG_WAIT_MINUTES: u32 = 1;
pub const MAX_RETRY_LONG_WAIT_MINUTES: u32 = 24 * 60;

/// The configured retry budget and stages (RD-1340-01).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetrySchedule {
    pub(crate) max_retries: u32,
    pub(crate) middle_attempts: u32,
    pub(crate) long_wait_minutes: u32,
}

impl RetrySchedule {
    /// The defaults with `max_retries` attempts.
    #[cfg(test)]
    pub(crate) const fn with_retries(max_retries: u32) -> Self {
        Self {
            max_retries,
            middle_attempts: DEFAULT_RETRY_MIDDLE_ATTEMPTS,
            long_wait_minutes: DEFAULT_RETRY_LONG_WAIT_MINUTES,
        }
    }

    /// The wait before the next attempt of a failure that is no hoster's limit, unspread: one,
    /// two, four, eight, sixteen seconds; then the middle stage's minutes, doubling and never
    /// above the long wait; then the long wait.
    fn backoff_ms(&self, previous_attempts: u32) -> u64 {
        if previous_attempts < FAST_RETRIES {
            return fast_backoff_ms(previous_attempts);
        }
        let long = u64::from(self.long_wait_minutes).saturating_mul(60_000);
        let step = previous_attempts - FAST_RETRIES;
        if step < self.middle_attempts {
            let first = MIDDLE_WAIT.as_millis().try_into().unwrap_or(u64::MAX);
            rd_core::exponential_backoff(step, first, long)
        } else {
            long
        }
    }
}

fn fast_backoff_ms(previous_attempts: u32) -> u64 {
    rd_core::exponential_backoff(
        previous_attempts,
        1_000,
        MAX_BACKOFF.as_millis().try_into().unwrap_or(u64::MAX),
    )
}

/// Attempts allowed for a rejected captcha, independent of the general retry count.
///
/// A rejected answer is usually an expired token and worth another try, but every attempt
/// buys a new solve from the configured service. Retrying it eight times like any transient
/// error would quietly spend eight times the money on one link, so this class stops early
/// and leaves a deliberate restart to the user.
const MAX_CAPTCHA_ATTEMPTS: u32 = 2;

/// Hold-off for a rate or daily limit the hoster put no duration on (RD-191-12). A daily
/// limit resets in hours, not minutes: on the generic backoff it was asked again every five
/// minutes and the download failed within the half hour.
pub const DEFAULT_RATE_LIMIT_WAIT: Duration = Duration::from_secs(60 * 60);

/// Consecutive waits for a hoster's limit before the download fails after all (RD-191-12).
///
/// A limit wait does not spend `max_retries`, so it needs a bound of its own: a hoster that
/// answers "limit reached" for good would otherwise hold the link forever. Forty-eight is two
/// days at the hour a stated-less rate limit waits, half a day at the IP block's quarter hour.
pub const MAX_LIMIT_WAITS: u32 = 48;

/// The failure a download carries once its limit waits ran out.
pub const LIMIT_WAITS_EXHAUSTED_CODE: &str = "download.limit_waits_exhausted";

/// Next retry time for a retryable failure, or `None` once its budget is spent.
///
/// Two budgets (RD-191-12): an attempt that went wrong spends one of `max_retries`; waiting
/// out a limit the hoster imposed spends one of [`MAX_LIMIT_WAITS`] and leaves the attempts
/// alone. A `max_retries` of zero switches automatic retries off, limit waits included.
pub(crate) fn retry_at(
    failure: &Failure,
    previous_attempts: u32,
    limit_waits: u32,
    schedule: RetrySchedule,
) -> Option<DateTime<Utc>> {
    let max_retries = schedule.max_retries;
    if !failure.category.is_retryable() || max_retries == 0 {
        return None;
    }
    if failure.category.is_limit() {
        if limit_waits >= MAX_LIMIT_WAITS {
            return None;
        }
    } else if previous_attempts >= max_retries {
        return None;
    }
    if failure.category == FailureKind::CaptchaFailed && previous_attempts >= MAX_CAPTCHA_ATTEMPTS {
        return None;
    }
    // The shared backoff and spread (re-audit 1.9.1, RA-TR-08): one, two, four, eight,
    // sixteen seconds, then the middle stage's minutes and the long wait (RD-1250-07,
    // RD-1340-01), each spread by ±`RETRY_JITTER_PERCENT` so links that failed together do not return
    // together. Seeded by the clock's sub-second part, as the notification retry is; the
    // spread is what matters, not unpredictability. A limit has its own waits below.
    let now = Utc::now();
    let backoff = if failure.category.is_limit() {
        fast_backoff_ms(previous_attempts)
    } else {
        schedule.backoff_ms(previous_attempts)
    };
    let backoff_ms = i64::try_from(backoff).unwrap_or(i64::MAX);
    let spread = rd_core::jitter(
        backoff_ms,
        rd_core::RETRY_JITTER_PERCENT,
        u64::from(now.timestamp_subsec_nanos()),
    );
    let calculated =
        Duration::from_millis(u64::try_from(backoff_ms.saturating_add(spread)).unwrap_or(0));
    // A server's or a plugin's wait, capped where it becomes a due time: an unbounded value
    // parked a download for years, and a large enough one overflowed the clock arithmetic and
    // panicked the task holding the download's slot (audit 1.9.1, TR-01).
    let stated = failure
        .category
        .retry_after()
        .map(|delay| Duration::from_secs(rd_core::clamp_retry_after(delay.as_secs())));
    let server = match (stated, &failure.category) {
        (Some(delay), _) => delay,
        (None, FailureKind::IpBlocked { .. }) => DEFAULT_IP_BLOCK,
        (None, FailureKind::RateLimited { .. }) => DEFAULT_RATE_LIMIT_WAIT,
        (None, _) => Duration::ZERO,
    };
    // A stated wait is kept as stated: the hoster named it, and the cap above is a day.
    chrono::Duration::from_std(calculated.max(server))
        .ok()
        .and_then(|delay| now.checked_add_signed(delay))
}

/// Whether a limit failure ended the download because its limit waits ran out.
pub(crate) fn limit_waits_spent(failure: &Failure, limit_waits: u32) -> bool {
    failure.category.is_limit() && limit_waits >= MAX_LIMIT_WAITS
}

/// The failure of a download whose limit waits ran out: the class is kept, so the automatic
/// retry of failed downloads still takes it up again later, and the hoster's own reason is
/// kept in `reason` and `reason_code`.
pub(crate) fn limit_waits_exhausted(original: Failure, waits: u32) -> Failure {
    let failure = Failure::coded(
        original.category.clone(),
        LIMIT_WAITS_EXHAUSTED_CODE,
        format!(
            "the hoster's limit was still in place after {waits} waits: {}",
            original.message
        ),
    )
    .with_param("waits", waits)
    .with_param("reason", original.message.clone());
    match original.code {
        Some(code) => failure.with_param("reason_code", code),
        None => failure,
    }
}

#[cfg(test)]
mod tests {
    use rd_core::{Failure, FailureKind};

    use super::{
        LIMIT_WAITS_EXHAUSTED_CODE, MAX_LIMIT_WAITS, RetrySchedule, limit_waits_exhausted,
        limit_waits_spent, retry_at,
    };

    fn transient() -> Failure {
        Failure::new(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            "flaky",
        )
    }

    #[test]
    fn retry_stops_at_configured_maximum() {
        assert!(retry_at(&transient(), 0, 0, RetrySchedule::with_retries(0)).is_none());
        assert!(retry_at(&transient(), 0, 0, RetrySchedule::with_retries(1)).is_some());
        assert!(retry_at(&transient(), 1, 0, RetrySchedule::with_retries(1)).is_none());
        assert!(retry_at(&transient(), 7, 0, RetrySchedule::with_retries(8)).is_some());
        assert!(retry_at(&transient(), 8, 0, RetrySchedule::with_retries(8)).is_none());
    }

    #[test]
    fn permanent_failures_never_retry() {
        let failure = Failure::new(FailureKind::Permanent, "gone");
        assert!(retry_at(&failure, 0, 0, RetrySchedule::with_retries(100)).is_none());
    }

    /// Every captcha retry buys another solve from the configured service, so this class
    /// stops well before the general retry count would.
    #[test]
    fn a_rejected_captcha_is_retried_only_a_couple_of_times() {
        let rejected = Failure::new(FailureKind::CaptchaFailed, "wrong captcha");

        assert!(retry_at(&rejected, 0, 0, RetrySchedule::with_retries(8)).is_some());
        assert!(retry_at(&rejected, 1, 0, RetrySchedule::with_retries(8)).is_some());
        assert!(
            retry_at(&rejected, 2, 0, RetrySchedule::with_retries(8)).is_none(),
            "a third paid solve for one link is not worth it"
        );
        // Other retryable classes keep the configured budget.
        assert!(retry_at(&transient(), 2, 0, RetrySchedule::with_retries(8)).is_some());
    }

    /// A free-download IP limit must not be retried on the generic backoff: without a
    /// server-supplied duration it waits out the default hold-off instead.
    #[test]
    fn an_ip_block_waits_much_longer_than_the_generic_backoff() {
        let blocked = Failure::new(
            FailureKind::IpBlocked {
                retry_after_seconds: None,
            },
            "one download at a time",
        );
        let due = retry_at(&blocked, 0, 0, RetrySchedule::with_retries(100))
            .expect("ip blocks are retryable");
        let delay = (due - chrono::Utc::now()).num_seconds();
        assert!(
            (14 * 60..=15 * 60 + 1).contains(&delay),
            "expected roughly the default hold-off, got {delay}s"
        );

        let hinted = Failure::new(
            FailureKind::IpBlocked {
                retry_after_seconds: Some(90),
            },
            "wait 90 seconds",
        );
        let due = retry_at(&hinted, 0, 0, RetrySchedule::with_retries(100))
            .expect("ip blocks are retryable");
        let delay = (due - chrono::Utc::now()).num_seconds();
        assert!(
            (89..=92).contains(&delay),
            "expected the hoster's own wait, got {delay}s"
        );
    }

    /// RA-TR-08: the shared backoff, spread by the shared percentage; from the sixth failed
    /// attempt in a row on the middle stage's one, two and four minutes, then half an hour
    /// (RD-1250-07, RD-1340-01).
    #[test]
    fn the_backoff_doubles_within_its_spread_then_waits_minutes_then_half_an_hour() {
        for (attempts, seconds) in [
            (0, 1),
            (1, 2),
            (3, 8),
            (4, 16),
            (5, 60),
            (6, 120),
            (7, 240),
            (8, 1_800),
            (40, 1_800),
        ] {
            let due = retry_at(&transient(), attempts, 0, RetrySchedule::with_retries(100))
                .expect("retryable");
            let delay = (due - chrono::Utc::now()).num_milliseconds();
            let base = seconds * 1_000;
            let spread = base * i64::from(rd_core::RETRY_JITTER_PERCENT) / 100;
            assert!(
                (base - spread - 50..=base + spread).contains(&delay),
                "attempt {attempts} waits {delay} ms, not {seconds} s within its spread"
            );
        }
    }

    /// RD-1340-01: the settings shape the stages. No middle stage goes from the fast retries
    /// straight to the long wait; a long wait shorter than a middle step caps it.
    #[test]
    fn the_middle_stage_and_the_long_wait_follow_the_settings() {
        let cases = [
            (
                RetrySchedule {
                    max_retries: 100,
                    middle_attempts: 0,
                    long_wait_minutes: 30,
                },
                5,
                1_800,
            ),
            (
                RetrySchedule {
                    max_retries: 100,
                    middle_attempts: 5,
                    long_wait_minutes: 10,
                },
                8,
                480,
            ),
            (
                RetrySchedule {
                    max_retries: 100,
                    middle_attempts: 5,
                    long_wait_minutes: 10,
                },
                9,
                600,
            ),
            (
                RetrySchedule {
                    max_retries: 100,
                    middle_attempts: 5,
                    long_wait_minutes: 10,
                },
                10,
                600,
            ),
        ];
        for (schedule, attempts, seconds) in cases {
            let due = retry_at(&transient(), attempts, 0, schedule).expect("retryable");
            let delay = (due - chrono::Utc::now()).num_milliseconds();
            let base = seconds * 1_000;
            let spread = base * i64::from(rd_core::RETRY_JITTER_PERCENT) / 100;
            assert!(
                (base - spread - 50..=base + spread).contains(&delay),
                "{schedule:?} attempt {attempts} waits {delay} ms, not {seconds} s"
            );
        }
    }

    /// TR-01: a `Retry-After` of any size becomes a due time within a day, never a panic and
    /// never a wait of years.
    #[test]
    fn a_huge_retry_after_is_capped_at_a_day() {
        for seconds in [
            u64::MAX,
            8_000_000_000_000,
            rd_core::MAX_RETRY_AFTER_SECONDS + 1,
        ] {
            let failure = Failure::new(
                FailureKind::RateLimited {
                    retry_after_seconds: Some(seconds),
                },
                "come back much later",
            );
            let due = retry_at(&failure, 0, 0, RetrySchedule::with_retries(8))
                .expect("a rate limit is retryable");
            let delay = (due - chrono::Utc::now()).num_seconds();
            let ceiling = i64::try_from(rd_core::MAX_RETRY_AFTER_SECONDS).expect("fits");
            assert!(
                (ceiling - 1..=ceiling + 2).contains(&delay),
                "Retry-After {seconds} became a wait of {delay}s"
            );
        }
    }

    fn daily_limit() -> Failure {
        Failure::new(
            FailureKind::RateLimited {
                retry_after_seconds: None,
            },
            "daily limit reached",
        )
    }

    /// RD-191-12: a limit the hoster imposed is waited out however many attempts went before;
    /// it does not spend `max_retries`.
    #[test]
    fn a_limit_wait_does_not_spend_the_attempts() {
        assert!(
            retry_at(&daily_limit(), 8, 0, RetrySchedule::with_retries(8)).is_some(),
            "a spent attempt budget ended a limit wait"
        );
        assert!(retry_at(&transient(), 8, 0, RetrySchedule::with_retries(8)).is_none());
        let blocked = Failure::new(
            FailureKind::IpBlocked {
                retry_after_seconds: None,
            },
            "one download at a time",
        );
        assert!(retry_at(&blocked, 100, 3, RetrySchedule::with_retries(100)).is_some());
    }

    /// RD-191-12: the limit waits have a bound of their own, and the failure then says so.
    #[test]
    fn the_limit_waits_end_after_their_own_bound() {
        assert!(
            retry_at(
                &daily_limit(),
                0,
                MAX_LIMIT_WAITS - 1,
                RetrySchedule::with_retries(8)
            )
            .is_some()
        );
        assert!(
            retry_at(
                &daily_limit(),
                0,
                MAX_LIMIT_WAITS,
                RetrySchedule::with_retries(8)
            )
            .is_none()
        );
        assert!(limit_waits_spent(&daily_limit(), MAX_LIMIT_WAITS));
        assert!(!limit_waits_spent(&daily_limit(), MAX_LIMIT_WAITS - 1));
        assert!(!limit_waits_spent(&transient(), MAX_LIMIT_WAITS));

        let ended = limit_waits_exhausted(
            Failure::coded(
                FailureKind::RateLimited {
                    retry_after_seconds: None,
                },
                "hoster.daily_limit",
                "daily limit reached",
            ),
            MAX_LIMIT_WAITS,
        );
        assert_eq!(ended.code.as_deref(), Some(LIMIT_WAITS_EXHAUSTED_CODE));
        assert!(ended.category.is_limit(), "the class was lost");
        assert_eq!(ended.params.get("waits").map(String::as_str), Some("48"));
        assert_eq!(
            ended.params.get("reason").map(String::as_str),
            Some("daily limit reached")
        );
        assert_eq!(
            ended.params.get("reason_code").map(String::as_str),
            Some("hoster.daily_limit")
        );
    }

    /// RD-191-12: a rate limit without a stated wait waits an hour, not the generic backoff.
    #[test]
    fn a_rate_limit_without_a_stated_wait_waits_an_hour() {
        let due = retry_at(&daily_limit(), 0, 0, RetrySchedule::with_retries(8))
            .expect("a rate limit is retryable");
        let delay = (due - chrono::Utc::now()).num_seconds();
        assert!(
            (59 * 60..=60 * 60 + 1).contains(&delay),
            "expected the hour, got {delay}s"
        );
    }

    /// Retries switched off are off for limits too.
    #[test]
    fn no_retries_means_no_limit_waits_either() {
        assert!(retry_at(&daily_limit(), 0, 0, RetrySchedule::with_retries(0)).is_none());
    }
}
