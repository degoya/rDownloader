use std::time::Duration;

use chrono::{DateTime, Utc};
use rand::Rng;
use rd_core::{Failure, FailureKind};

/// Upper bound accepted for the configurable retry count.
pub const MAX_CONFIGURABLE_RETRIES: u32 = 100;

/// Hold-off applied when a hoster reports an IP limit without naming a duration. Free
/// download limits are counted in tens of minutes, so the exponential backoff (five
/// minutes at most) would retry far too eagerly.
const DEFAULT_IP_BLOCK: Duration = Duration::from_secs(15 * 60);

/// Attempts allowed for a rejected captcha, independent of the general retry count.
///
/// A rejected answer is usually an expired token and worth another try, but every attempt
/// buys a new solve from the configured service. Retrying it eight times like any transient
/// error would quietly spend eight times the money on one link, so this class stops early
/// and leaves a deliberate restart to the user.
const MAX_CAPTCHA_ATTEMPTS: u32 = 2;

/// Next retry time for a retryable failure, or `None` once `max_retries` attempts were used.
pub(crate) fn retry_at(
    failure: &Failure,
    previous_attempts: u32,
    max_retries: u32,
) -> Option<DateTime<Utc>> {
    if !failure.category.is_retryable() || previous_attempts >= max_retries {
        return None;
    }
    if failure.category == FailureKind::CaptchaFailed && previous_attempts >= MAX_CAPTCHA_ATTEMPTS {
        return None;
    }
    let exponent = previous_attempts.min(8);
    let calculated = Duration::from_secs(2_u64.pow(exponent).min(300));
    let stated = failure.category.retry_after();
    let server = match (stated, &failure.category) {
        (Some(delay), _) => delay,
        (None, FailureKind::IpBlocked { .. }) => DEFAULT_IP_BLOCK,
        (None, _) => Duration::ZERO,
    };
    let jitter = Duration::from_millis(rand::rng().random_range(0..=1_000));
    chrono::Duration::from_std(calculated.max(server).saturating_add(jitter))
        .ok()
        .map(|delay| Utc::now() + delay)
}

#[cfg(test)]
mod tests {
    use rd_core::{Failure, FailureKind};

    use super::retry_at;

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
        assert!(retry_at(&transient(), 0, 0).is_none());
        assert!(retry_at(&transient(), 0, 1).is_some());
        assert!(retry_at(&transient(), 1, 1).is_none());
        assert!(retry_at(&transient(), 7, 8).is_some());
        assert!(retry_at(&transient(), 8, 8).is_none());
    }

    #[test]
    fn permanent_failures_never_retry() {
        let failure = Failure::new(FailureKind::Permanent, "gone");
        assert!(retry_at(&failure, 0, 100).is_none());
    }

    /// Every captcha retry buys another solve from the configured service, so this class
    /// stops well before the general retry count would.
    #[test]
    fn a_rejected_captcha_is_retried_only_a_couple_of_times() {
        let rejected = Failure::new(FailureKind::CaptchaFailed, "wrong captcha");

        assert!(retry_at(&rejected, 0, 8).is_some());
        assert!(retry_at(&rejected, 1, 8).is_some());
        assert!(
            retry_at(&rejected, 2, 8).is_none(),
            "a third paid solve for one link is not worth it"
        );
        // Other retryable classes keep the configured budget.
        assert!(retry_at(&transient(), 2, 8).is_some());
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
        let due = retry_at(&blocked, 0, 100).expect("ip blocks are retryable");
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
        let due = retry_at(&hinted, 0, 100).expect("ip blocks are retryable");
        let delay = (due - chrono::Utc::now()).num_seconds();
        assert!(
            (89..=92).contains(&delay),
            "expected the hoster's own wait, got {delay}s"
        );
    }
}
