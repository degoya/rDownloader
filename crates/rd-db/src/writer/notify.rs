//! Reacting to what happened: the writer half of `notify_store` and `automation_store`.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_notify(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::UpsertNotificationTarget { id, input, reply } => {
                let result =
                    crate::notify_store::upsert_target(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteNotificationTarget { id, reply } => {
                let result = crate::notify_store::delete_target(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpsertNotificationRule { id, input, reply } => {
                let result =
                    crate::notify_store::upsert_rule(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteNotificationRule { id, reply } => {
                let result = crate::notify_store::delete_rule(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::QueueNotificationDelivery { input, reply } => {
                // Queuing is silent: the delivery worker broadcasts once it has a result.
                let result = crate::notify_store::queue_delivery(&mut self.connection, input).await;
                send(reply, result);
            }
            WriterCommand::RecordNotificationAttempt {
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
            WriterCommand::ClearNotificationDeliveries { reply } => {
                let result = crate::notify_store::clear_deliveries(&mut self.connection).await;
                send(reply, result);
            }
            WriterCommand::UpsertAutomation { id, input, reply } => {
                let result = crate::automation_store::upsert(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetAutomationEnabled { id, enabled, reply } => {
                let result =
                    crate::automation_store::set_enabled(&mut self.connection, id, enabled).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteAutomation { id, reply } => {
                let result = crate::automation_store::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::QueueAutomationRun { input, reply } => {
                // Queuing is silent; the engine reports once a run has an outcome.
                let result = crate::automation_store::queue_run(&mut self.connection, input).await;
                send(reply, result);
            }
            WriterCommand::RecordAutomationAttempt {
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
            WriterCommand::RecoverAutomationRuns { reply } => {
                let result = crate::automation_store::recover(&mut self.connection).await;
                send(reply, result);
            }
            // `Writer::run` routes every variant to exactly one handler, and its match is
            // exhaustive over `WriterCommand`, so nothing reaches this arm. It drops the
            // command instead of panicking: a mis-routed command must not take down the one
            // task every mutation in the process runs on, and the caller already treats a
            // dropped reply as a failed request.
            _ => {}
        }
    }
}
