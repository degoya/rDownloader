//! The writer half of `subscription_store`: subscriptions, their items and their runs.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_subscriptions(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::CreateSubscription { input, reply } => {
                let result = crate::subscription_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateSubscription { id, input, reply } => {
                let result = crate::subscription_store::update(&mut self.connection, id, *input)
                    .await
                    .map(|(value, orphan, event)| {
                        let _ = self.events.send(event);
                        (value, orphan)
                    });
                send(reply, result);
            }
            WriterCommand::SetSubscriptionEnabled { id, enabled, reply } => {
                let result =
                    crate::subscription_store::set_enabled(&mut self.connection, id, enabled).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteSubscription { id, reply } => {
                let result = crate::subscription_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::RecordSubscriptionItems {
                subscription_id,
                items,
                reply,
            } => {
                let result = crate::subscription_store::record_items(
                    &mut self.connection,
                    subscription_id,
                    items,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetSubscriptionItemState { id, state, reply } => {
                let result =
                    crate::subscription_store::set_item_state(&mut self.connection, id, state)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::SetPendingSubscriptionItemsState { ids, state, reply } => {
                let result = crate::subscription_store::set_pending_items_state(
                    &mut self.connection,
                    &ids,
                    state,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::ClearSubscriptionHistory { id, reply } => {
                let result =
                    crate::subscription_store::clear_history(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::ArmSubscription {
                id,
                next_run_at,
                reply,
            } => {
                let result =
                    crate::subscription_store::arm(&mut self.connection, id, next_run_at).await;
                send(reply, result);
            }
            WriterCommand::FinishSubscriptionRun {
                subscription_id,
                started_at,
                result,
                reply,
            } => {
                let outcome = crate::subscription_store::finish_run(
                    &mut self.connection,
                    subscription_id,
                    started_at,
                    *result,
                )
                .await;
                publish_unit_event(reply, outcome, &self.events);
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
