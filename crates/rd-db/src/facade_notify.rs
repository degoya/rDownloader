//! Database facade for notification targets, rules and the delivery history (RD-050-14).

use anyhow::Result;

use crate::{Database, commands::NotifyCommand, notify_store, writer};

/// Notification targets, rules and the delivery history (RD-050-14).
impl Database {
    pub async fn list_notification_targets(&self) -> Result<Vec<rd_notify::NotificationTarget>> {
        notify_store::list_targets(&self.readers).await
    }

    pub async fn list_notification_rules(&self) -> Result<Vec<rd_notify::NotificationRule>> {
        notify_store::list_rules(&self.readers).await
    }

    pub async fn list_notification_deliveries(
        &self,
        limit: u32,
    ) -> Result<Vec<rd_notify::Delivery>> {
        notify_store::list_deliveries(&self.readers, limit).await
    }

    /// How many deliveries a clear of the history would remove right now: every one except
    /// those still queued or retrying (RD-130-08).
    pub async fn count_clearable_notification_deliveries(&self) -> Result<u64> {
        notify_store::count_clearable_deliveries(&self.readers).await
    }

    /// How many deliveries are still queued or retrying: what discarding the pending
    /// notifications would remove (RD-170-11).
    pub async fn count_pending_notification_deliveries(&self) -> Result<u64> {
        notify_store::count_pending_deliveries(&self.readers).await
    }

    /// Deliveries whose next attempt is due.
    pub async fn due_notification_deliveries(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_notify::Delivery>> {
        notify_store::due_deliveries(&self.readers, now).await
    }

    pub async fn upsert_notification_target(
        &self,
        id: Option<rd_core::NotificationTargetId>,
        input: notify_store::NewNotificationTarget,
    ) -> Result<rd_notify::NotificationTarget> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::UpsertNotificationTarget { id, input, reply }
        })
        .await
    }

    /// Removes a target and returns its vault reference, so the caller can drop the secret.
    pub async fn delete_notification_target(
        &self,
        id: rd_core::NotificationTargetId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::DeleteNotificationTarget { id, reply }
        })
        .await
    }

    pub async fn upsert_notification_rule(
        &self,
        id: Option<rd_core::NotificationRuleId>,
        input: notify_store::NewNotificationRule,
    ) -> Result<rd_notify::NotificationRule> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::UpsertNotificationRule { id, input, reply }
        })
        .await
    }

    pub async fn delete_notification_rule(&self, id: rd_core::NotificationRuleId) -> Result<()> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::DeleteNotificationRule { id, reply }
        })
        .await
    }

    /// Queues a delivery; `false` means the idempotency key was already taken.
    pub async fn queue_notification_delivery(
        &self,
        input: notify_store::NewDelivery,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::QueueNotificationDelivery { input, reply }
        })
        .await
    }

    /// Queues an operational notice's deliveries (RD-190-19). A delivery whose idempotency key
    /// a notice queued before is skipped, however long ago and whatever became of the delivery;
    /// returns how many were queued.
    pub async fn queue_notification_notice(
        &self,
        deliveries: Vec<notify_store::NewDelivery>,
    ) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::QueueNotificationNotice { deliveries, reply }
        })
        .await
    }

    pub async fn record_notification_attempt(
        &self,
        id: rd_core::NotificationDeliveryId,
        state: rd_notify::DeliveryState,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        response_status: Option<u16>,
        response_excerpt: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::RecordNotificationAttempt {
                id,
                state,
                attempt,
                next_attempt_at,
                response_status,
                response_excerpt,
                reply,
            }
        })
        .await
    }

    /// Empties the delivery history and reports how many rows went. Deliveries still queued
    /// or retrying stay: the worker owes them an attempt (RD-130-08).
    pub async fn clear_notification_deliveries(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::ClearNotificationDeliveries { reply }
        })
        .await
    }

    /// Cancels every notification still queued or retrying by deleting its delivery, and
    /// reports how many went (RD-170-11). The finished history stays.
    pub async fn discard_pending_notification_deliveries(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            NotifyCommand::DiscardPendingNotificationDeliveries { reply }
        })
        .await
    }
}
