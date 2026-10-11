//! Delivering through a `web_push` target (RD-1240-13): one push to every browser that turned
//! push on and wants the event.

use super::*;

impl NotificationService {
    /// Pushes `message` to the subscribed browsers and deletes the subscriptions a push service
    /// answered 404 or 410 for. The test action (`every_browser`) reaches every browser, whatever
    /// events it chose.
    pub(super) async fn deliver_web_push(&self, message: &Message, every_browser: bool) -> Attempt {
        let database = &self.inner.database;
        let subscriptions = match database.list_web_push_subscriptions().await {
            Ok(subscriptions) => subscriptions,
            Err(error) => return Attempt::could_not_deliver(error.to_string(), true),
        };
        if subscriptions.is_empty() {
            return Attempt::could_not_deliver(
                "no browser has push turned on; turn it on under Settings > Interface",
                false,
            );
        }
        let wanted: Vec<_> = subscriptions
            .iter()
            .filter(|subscription| every_browser || subscription.wants(message.event))
            .collect();
        // Every browser chose other events: nothing to send is nothing that failed.
        if wanted.is_empty() {
            return Attempt::succeeded();
        }
        let key = match crate::web_push::vapid_key(database, &self.inner.secrets).await {
            Ok(key) => key,
            Err(error) => return Attempt::could_not_deliver(error.to_string(), true),
        };
        let payload = rd_notify::push_payload(message);
        let severity = message.event.severity();
        let mut outcomes = Vec::with_capacity(wanted.len());
        for subscription in wanted {
            let outcome = rd_notify::send_push(&key, subscription, &payload, severity).await;
            outcomes.push((subscription, outcome));
        }
        let (attempt, gone) = crate::web_push::settle_pushes(outcomes);
        for id in gone {
            // Already deleted by hand in the meantime is just as gone.
            if let Err(error) = database.delete_web_push_subscription(&id).await
                && rd_db::store_kind(&error) != Some(rd_db::StoreErrorKind::NotFound)
            {
                tracing::warn!(%error, "an expired push subscription could not be deleted");
            }
        }
        attempt
    }
}
