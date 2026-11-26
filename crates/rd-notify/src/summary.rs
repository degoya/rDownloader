//! What a delivery says, and what the deliveries quiet hours held back say together once they
//! end (RD-1260-04): one message per target instead of the flood the queue had been saving up.

use crate::{Delivery, Message, NotificationLanguage, Subject, Text};

/// How many of the held titles a summary names; the rest it counts.
pub const SUMMARY_NAMED: usize = 5;

/// The message one delivery carries.
#[must_use]
pub fn message_of(delivery: &Delivery) -> Message {
    let mut payload = serde_json::json!({
        "event": delivery.event,
        "title": delivery.title,
        "body": delivery.body,
        "idempotency_key": delivery.idempotency_key,
    });
    Subject {
        occurred_at: Some(occurred_at(delivery)),
        ..delivery.subject
    }
    .describe(&mut payload);
    Message {
        title: delivery.title.clone(),
        body: delivery.body.clone(),
        event: delivery.event,
        idempotency_key: delivery.idempotency_key.clone(),
        payload,
    }
}

/// When it happened; a delivery queued before 1.48 knows only when it was queued.
fn occurred_at(delivery: &Delivery) -> rd_core::Timestamp {
    delivery.subject.occurred_at.unwrap_or(delivery.created_at)
}

/// One message for every delivery in `held`, or the delivery's own when there is one; `None`
/// for none. It carries the most serious of their events, so a rule's or a browser's choice
/// of events lets the summary through when it would have let the worst of them through. Its
/// own words are in `language`, and its `occurred_at` is the first held one's; it names no
/// single package.
#[must_use]
pub fn quiet_summary(held: &[Delivery], language: NotificationLanguage) -> Option<Message> {
    let first = held.first()?;
    if held.len() == 1 {
        return Some(message_of(first));
    }
    let event = held
        .iter()
        .map(|delivery| delivery.event)
        .max_by_key(|event| event.severity())
        .unwrap_or(first.event);
    let count = u64::try_from(held.len()).unwrap_or(u64::MAX);
    let title = Text::new("quiet_summary.title")
        .with("count", count)
        .say(language);
    let named: Vec<&str> = held
        .iter()
        .take(SUMMARY_NAMED)
        .map(|delivery| delivery.title.as_str())
        .collect();
    let rest = held.len().saturating_sub(SUMMARY_NAMED);
    let body = if rest == 0 {
        format!("{}.", named.join("; "))
    } else {
        Text::new("quiet_summary.body_more")
            .with("titles", named.join("; "))
            .with("count", u64::try_from(rest).unwrap_or(u64::MAX))
            .say(language)
    };
    let idempotency_key = format!("quiet-summary:{}", first.idempotency_key);
    let mut payload = serde_json::json!({
        "event": event,
        "title": title,
        "body": body,
        "idempotency_key": idempotency_key,
        "count": held.len(),
    });
    Subject {
        occurred_at: held.iter().map(occurred_at).min(),
        ..Subject::default()
    }
    .describe(&mut payload);
    Some(Message {
        payload,
        title,
        body,
        event,
        idempotency_key,
    })
}

#[cfg(test)]
mod tests {
    use rd_core::{NotificationDeliveryId, NotificationRuleId, NotificationTargetId, Timestamp};

    use super::{SUMMARY_NAMED, message_of, quiet_summary};
    use crate::{Delivery, DeliveryState, NotificationEvent, NotificationLanguage as L, Subject};

    fn held(index: usize, event: NotificationEvent) -> Delivery {
        let now = Timestamp::now();
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
            subject: Subject::default(),
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
        let summary = quiet_summary(&night, L::En).expect("a summary");
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
        let message = quiet_summary(&one, L::En).expect("the message");
        assert_eq!(message.title, "Package finished: 7");
        assert_eq!(message.idempotency_key, "rule:evt-7");
        assert!(quiet_summary(&[], L::En).is_none());
    }

    /// RD-1480-11: the summary's own words are in the chosen language, the held titles as they
    /// were queued; the body says when the first of them happened.
    #[test]
    fn a_summary_speaks_the_chosen_language() {
        let mut night: Vec<_> = (0..7)
            .map(|index| held(index, NotificationEvent::PackageCompleted))
            .collect();
        let first = Timestamp::now() - rd_core::SignedDuration::from_secs(3600);
        night[3].subject.occurred_at = Some(first);
        let summary = quiet_summary(&night, L::De).expect("a summary");
        let title = crate::Text::new("quiet_summary.title").with("count", 7_u64);
        assert_eq!(summary.title, title.say(L::De));
        assert_ne!(summary.title, title.say(L::En));
        assert!(!summary.body.contains("more"), "{}", summary.body);
        assert!(
            summary.body.starts_with("Package finished: 0; "),
            "{}",
            summary.body
        );
        assert_eq!(summary.payload["version"], 1);
        assert_eq!(summary.payload["occurred_at"], serde_json::json!(first));
        assert!(summary.payload["package_id"].is_null());
    }

    /// RD-1480-11: the body keeps its four fields and adds when it happened and to what; a
    /// delivery queued before that was kept says when it was queued.
    #[test]
    fn a_message_says_when_it_happened_and_to_what() {
        let mut delivery = held(1, NotificationEvent::PackageCompleted);
        let old = message_of(&delivery).payload;
        assert_eq!(old["occurred_at"], serde_json::json!(delivery.created_at));
        assert!(old["package_id"].is_null() && old["download_id"].is_null());

        let package = rd_core::PackageId::new();
        let download = rd_core::DownloadId::new();
        let at = delivery.created_at - rd_core::SignedDuration::from_secs(60);
        delivery.subject = Subject {
            occurred_at: Some(at),
            package_id: Some(package),
            download_id: Some(download),
        };
        let payload = message_of(&delivery).payload;
        assert_eq!(
            payload,
            serde_json::json!({
                "version": 1,
                "event": "package_completed",
                "title": "Package finished: 1",
                "body": "",
                "idempotency_key": "rule:evt-1",
                "occurred_at": at,
                "package_id": package,
                "download_id": download,
            })
        );
    }
}
