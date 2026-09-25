//! The one decision about a cron schedule the poll loop makes before it polls (RD-130-19).
//!
//! Split out of `subscription_service.rs` for size; the method belongs to the same service.

use rd_core::Subscription;

use super::SubscriptionService;

impl SubscriptionService {
    /// Times a scheduled subscription that has never been timed, instead of running it.
    ///
    /// A new subscription without a schedule is due at once, so its backlog decision shows
    /// immediately. One with `0 6 * * *` means six o'clock, not "now, and then at six": it is
    /// given its first time here and left alone until then. Returns whether it was armed; an
    /// expression that names no time is not, so the poll runs and fails with the reason.
    pub(super) async fn arm(
        &self,
        subscription: &Subscription,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        let (Some(expression), None) = (&subscription.schedule, subscription.next_run_at) else {
            return false;
        };
        let Ok(next) = rd_subscription::next_scheduled(expression, now, &chrono::Local) else {
            return false;
        };
        if let Err(error) = self
            .inner
            .database
            .arm_subscription(subscription.id, next)
            .await
        {
            tracing::warn!(subscription = %subscription.name, %error, "schedule could not be armed");
        }
        true
    }
}
