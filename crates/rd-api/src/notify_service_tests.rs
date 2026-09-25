//! The retry decision for plugin failures and the `budget_exhausted` mapping (RD-120-62).

use rd_core::{Failure, FailureKind};
use rd_notify::{Attempt, DeliveryState, NotificationEvent};

use super::{budget_exhausted, plugin_failure, settle};

/// What the host hands back for a failure the plugin reported: the `rd_core::Failure` inside
/// the error, exactly as `NotifierPlugin::deliver` builds it.
fn reported(category: FailureKind, message: &str) -> anyhow::Error {
    anyhow::Error::new(Failure::new(category, message))
}

#[test]
fn a_failure_the_plugin_calls_permanent_fails_at_once_with_its_reason() {
    let now = chrono::Utc::now();
    for category in [
        FailureKind::Permanent,
        FailureKind::AuthRequired,
        FailureKind::AccountInvalid,
        FailureKind::Unsupported,
    ] {
        let attempt = plugin_failure(&reported(category.clone(), "Unauthorized: bot token"));
        assert!(!attempt.ok);
        assert!(!attempt.retryable, "{category:?} must not be retried");
        assert_eq!(attempt.excerpt.as_deref(), Some("Unauthorized: bot token"));
        // The first attempt is the last one: no backoff, no second try half an hour later.
        assert_eq!(settle(&attempt, 1, now), (DeliveryState::Failed, None));
    }
}

#[test]
fn a_failure_the_plugin_calls_temporary_is_still_retried() {
    let now = chrono::Utc::now();
    for category in [
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        FailureKind::RateLimited {
            retry_after_seconds: Some(30),
        },
        FailureKind::Offline,
    ] {
        let attempt = plugin_failure(&reported(category.clone(), "HTTP 503"));
        assert!(attempt.retryable, "{category:?} must be retried");
        let (state, next) = settle(&attempt, 1, now);
        assert_eq!(state, DeliveryState::Retrying);
        assert!(next.is_some_and(|next| next > now));
    }
    // Past the attempt limit even a retryable failure gives up.
    let attempt = plugin_failure(&reported(FailureKind::Offline, "HTTP 503"));
    assert_eq!(
        settle(&attempt, rd_notify::MAX_ATTEMPTS, now),
        (DeliveryState::Failed, None)
    );
}

#[test]
fn an_error_without_a_category_stays_retryable() {
    // A trap or a component that would not instantiate says nothing about the destination.
    let attempt = plugin_failure(&anyhow::anyhow!("wasm trap: out of fuel"));
    assert!(attempt.retryable);
    assert_eq!(
        settle(&Attempt::succeeded(), 1, chrono::Utc::now()),
        (DeliveryState::Delivered, None)
    );
}

#[test]
fn only_a_budget_running_out_becomes_budget_exhausted() {
    let profile = rd_core::BandwidthProfileId::new();
    let (event, category, title, body) = budget_exhausted(&serde_json::json!({
        "entity": "budget",
        "exhausted": true,
        "profile": profile,
        "period": "monthly",
        "used_bytes": 600,
        "limit_bytes": 500
    }))
    .expect("a used-up budget notifies");
    assert_eq!(event, NotificationEvent::BudgetExhausted);
    assert_eq!(category, None);
    assert_eq!(title, "Traffic budget used up");
    assert!(
        body.contains("monthly") && body.contains("500 bytes"),
        "{body}"
    );

    for quiet in [
        serde_json::json!({ "entity": "budget", "exhausted": false, "profile": profile }),
        serde_json::json!({ "entity": "profile", "id": profile }),
        serde_json::json!({ "entity": "schedule" }),
        serde_json::json!({ "entity": "active", "profile": profile }),
    ] {
        assert!(budget_exhausted(&quiet).is_none(), "{quiet}");
    }
}
