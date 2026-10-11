//! Server-side notifications: filterable rules, signed webhooks, SMTP and Apprise-compatible
//! targets with a persistent delivery history (RD-050-14), and Web Push to the installed app's
//! browsers (RD-1240-13).
//!
//! The crate owns the contract and the transports. Persisting deliveries and running the
//! worker is the service's job, so the policy stays testable without a database.

#![warn(unreachable_pub)]

mod coalesce;
mod delivery;
mod executable;
mod model;
mod retry;

pub use coalesce::{Burst, Coalescer, LONGEST_SECONDS, NAMED, Occurrence, QUIET_SECONDS};
pub use delivery::{
    Attempt, IDEMPOTENCY_HEADER, Message, SIGNATURE_HEADER, TargetConfig, send,
    web_push::{
        PushOutcome, VapidKey, WebPushSubscription, are_push_keys, is_deliverable_push_address,
        is_push_address, push_payload, send_push,
    },
};
pub use executable::{EXECUTABLE_SEAL, ExecutableNeedsAdmin, check_executable, seal_executable};
pub use model::{
    Delivery, DeliveryState, NotificationEvent, NotificationRule, NotificationTarget, Severity,
    TargetKind,
};
pub use retry::{JITTER_PERCENT, MAX_ATTEMPTS, backoff, backoff_with_seed, next_attempt_at};

/// Builds the idempotency key of a delivery.
///
/// Derived from the rule and the event rather than randomly generated, so replaying the same
/// event after a crash produces the same key and the unique index drops the duplicate.
#[must_use]
pub fn idempotency_key(rule_id: rd_core::NotificationRuleId, event_id: &str) -> String {
    format!("{rule_id}:{event_id}")
}

#[cfg(test)]
mod tests {
    use super::idempotency_key;

    #[test]
    fn the_same_rule_and_event_always_produce_the_same_key() {
        let rule = rd_core::NotificationRuleId::new();
        assert_eq!(
            idempotency_key(rule, "evt-1"),
            idempotency_key(rule, "evt-1")
        );
        assert_ne!(
            idempotency_key(rule, "evt-1"),
            idempotency_key(rule, "evt-2")
        );
        let other = rd_core::NotificationRuleId::new();
        assert_ne!(
            idempotency_key(rule, "evt-1"),
            idempotency_key(other, "evt-1")
        );
    }
}
