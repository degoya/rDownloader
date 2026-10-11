//! Reacting to what happened: the writer half of `notify_store` and `automation_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::NotifyCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_notify(&mut self, command: NotifyCommand) {
        match command {
            NotifyCommand::UpsertNotificationTarget { id, input, reply } => {
                let result =
                    crate::notify_store::upsert_target(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            NotifyCommand::DeleteNotificationTarget { id, reply } => {
                let result = crate::notify_store::delete_target(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            NotifyCommand::UpsertNotificationRule { id, input, reply } => {
                let result =
                    crate::notify_store::upsert_rule(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            NotifyCommand::DeleteNotificationRule { id, reply } => {
                let result = crate::notify_store::delete_rule(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            NotifyCommand::QueueNotificationDelivery { input, reply } => {
                // Queuing is silent: the delivery worker broadcasts once it has a result.
                let result = crate::notify_store::queue_delivery(&mut self.connection, input).await;
                send(reply, result);
            }
            NotifyCommand::QueueNotificationNotice { deliveries, reply } => {
                // Silent like a single delivery.
                let result =
                    crate::notice_store::queue_notice(&mut self.connection, deliveries).await;
                send(reply, result);
            }
            NotifyCommand::RecordNotificationAttempt {
                id,
                state,
                attempt,
                next_attempt_at,
                response_status,
                response_excerpt,
                reply,
            } => {
                let result = crate::notify_store::record_attempt(
                    &mut self.connection,
                    id,
                    state,
                    attempt,
                    next_attempt_at,
                    response_status,
                    response_excerpt,
                )
                .await;
                send(reply, result);
            }
            NotifyCommand::ClearNotificationDeliveries { reply } => {
                let result = crate::notify_store::clear_deliveries(&mut self.connection).await;
                send(reply, result);
            }
            NotifyCommand::DiscardPendingNotificationDeliveries { reply } => {
                let result =
                    crate::notify_store::discard_pending_deliveries(&mut self.connection).await;
                send(reply, result);
            }
            NotifyCommand::UpsertWebPushSubscription { input, reply } => {
                let result =
                    crate::web_push_store::upsert_subscription(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            NotifyCommand::DeleteWebPushSubscription { id, reply } => {
                let result =
                    crate::web_push_store::delete_subscription(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            NotifyCommand::StoreWebPushKey {
                key,
                replacing,
                reply,
            } => {
                let result =
                    crate::web_push_store::store_key(&mut self.connection, key, replacing).await;
                send(reply, result);
            }
            NotifyCommand::UpsertAutomation { id, input, reply } => {
                let result = crate::automation_store::upsert(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            NotifyCommand::SetAutomationEnabled { id, enabled, reply } => {
                let result =
                    crate::automation_store::set_enabled(&mut self.connection, id, enabled).await;
                publish_config(reply, result, &self.events);
            }
            NotifyCommand::DeleteAutomation { id, reply } => {
                let result = crate::automation_store::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            NotifyCommand::QueueAutomationRun { input, reply } => {
                // Queuing is silent; the engine reports once a run has an outcome.
                let result = crate::automation_store::queue_run(&mut self.connection, input).await;
                send(reply, result);
            }
            NotifyCommand::RecordAutomationAttempt {
                id,
                state,
                action_index,
                attempt,
                next_attempt_at,
                message,
                reply,
            } => {
                let result = crate::automation_store::record_attempt(
                    &mut self.connection,
                    id,
                    state,
                    action_index,
                    attempt,
                    next_attempt_at,
                    message,
                )
                .await;
                send(reply, result);
            }
            NotifyCommand::RecoverAutomationRuns { reply } => {
                let result = crate::automation_store::recover(&mut self.connection).await;
                send(reply, result);
            }
        }
    }
}
