//! What a delivery says, and what the deliveries quiet hours held back say together once they
//! end (RD-1260-04): one message per target instead of the flood the queue had been saving up.

use crate::{Delivery, Message};

/// How many of the held titles a summary names; the rest it counts.
pub const SUMMARY_NAMED: usize = 5;

/// The message one delivery carries.
#[must_use]
pub fn message_of(delivery: &Delivery) -> Message {
    Message {
        title: delivery.title.clone(),
        body: delivery.body.clone(),
        event: delivery.event,
        idempotency_key: delivery.idempotency_key.clone(),
        payload: serde_json::json!({
            "event": delivery.event,
            "title": delivery.title,
            "body": delivery.body,
            "idempotency_key": delivery.idempotency_key,
        }),
    }
}

/// One message for every delivery in `held`, or the delivery's own when there is one; `None`
/// for none. It carries the most serious of their events, so a rule's or a browser's choice
/// of events lets the summary through when it would have let the worst of them through.
#[must_use]
pub fn quiet_summary(held: &[Delivery]) -> Option<Message> {
    let first = held.first()?;
    if held.len() == 1 {
        return Some(message_of(first));
    }
    let event = held
        .iter()
        .map(|delivery| delivery.event)
        .max_by_key(|event| event.severity())
        .unwrap_or(first.event);
    let title = format!("While it was quiet: {} notifications", held.len());
    let named: Vec<&str> = held
        .iter()
        .take(SUMMARY_NAMED)
        .map(|delivery| delivery.title.as_str())
        .collect();
    let rest = held.len().saturating_sub(SUMMARY_NAMED);
    let body = if rest == 0 {
        format!("{}.", named.join("; "))
    } else {
        format!("{}; and {rest} more.", named.join("; "))
    };
    let idempotency_key = format!("quiet-summary:{}", first.idempotency_key);
    Some(Message {
        payload: serde_json::json!({
            "event": event,
            "title": title,
            "body": body,
            "idempotency_key": idempotency_key,
            "count": held.len(),
        }),
        title,
        body,
        event,
        idempotency_key,
    })
}

#[cfg(test)]
mod tests {
    use rd_core::{NotificationDeliveryId, NotificationRuleId, NotificationTargetId};

    use super::{SUMMARY_NAMED, quiet_summary};
    use crate::{Delivery, DeliveryState, NotificationEvent};

    fn held(index: usize, event: NotificationEvent) -> Delivery {
        let now = chrono::Utc::now();
        Delivery {
            id: NotificationDeliveryId::new(),
            rule_id: NotificationRuleId::new(),
            target_id: NotificationTargetId::new(),
            idempotency_key: format!("rule:evt-{index}"),
            event,
            title: format!("Package finished: {index}"),
            body: String::new(),
            state: DeliveryState::Queued,
            attempt: 0,
            next_attempt_at: None,
            response_status: None,
            response_excerpt: None,
            created_at: now,
            updated_at: now,
            package_path: None,
        }
    }

    /// Twenty packages finished and one failed during the night: one message in the morning,
    /// as serious as the failure, naming the first few and counting the rest.
    #[test]
    fn a_night_of_notifications_is_one_message() {
        let mut night: Vec<_> = (0..20)
            .map(|index| held(index, NotificationEvent::PackageCompleted))
            .collect();
        night.push(held(20, NotificationEvent::PackageFailed));
        let summary = quiet_summary(&night).expect("a summary");
        assert_eq!(summary.title, "While it was quiet: 21 notifications");
        assert_eq!(summary.event, NotificationEvent::PackageFailed);
        assert!(
            summary.body.starts_with("Package finished: 0; "),
            "{}",
            summary.body
        );
        assert!(
            summary
                .body
                .ends_with(&format!("and {} more.", 21 - SUMMARY_NAMED)),
            "{}",
            summary.body
        );
        assert_eq!(summary.idempotency_key, "quiet-summary:rule:evt-0");
        assert_eq!(summary.payload["count"], 21);
    }

    #[test]
    fn one_held_delivery_is_sent_as_itself_and_none_is_nothing() {
        let one = [held(7, NotificationEvent::StorageBlocked)];
        let message = quiet_summary(&one).expect("the message");
        assert_eq!(message.title, "Package finished: 7");
        assert_eq!(message.idempotency_key, "rule:evt-7");
        assert!(quiet_summary(&[]).is_none());
    }
}
