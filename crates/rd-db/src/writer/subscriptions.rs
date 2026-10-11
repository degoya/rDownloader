//! The writer half of `subscription_store`: subscriptions, their items and their runs.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::SubscriptionsCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_subscriptions(&mut self, command: SubscriptionsCommand) {
        match command {
            SubscriptionsCommand::CreateSubscription { input, reply } => {
                let result = crate::subscription_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            SubscriptionsCommand::UpdateSubscription { id, input, reply } => {
                let result = crate::subscription_store::update(&mut self.connection, id, *input)
                    .await
                    .map(|(value, orphan, event)| {
                        let _ = self.events.send(event);
                        (value, orphan)
                    });
                send(reply, result);
            }
            SubscriptionsCommand::SetSubscriptionEnabled { id, enabled, reply } => {
                let result =
                    crate::subscription_store::set_enabled(&mut self.connection, id, enabled).await;
                publish_config(reply, result, &self.events);
            }
            SubscriptionsCommand::DeleteSubscription { id, reply } => {
                let result = crate::subscription_store::delete(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            SubscriptionsCommand::RecordSubscriptionItems {
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
            SubscriptionsCommand::SetSubscriptionItemState { id, state, reply } => {
                let result =
                    crate::subscription_store::set_item_state(&mut self.connection, id, state)
                        .await;
                publish_unit_event(reply, result, &self.events);
            }
            SubscriptionsCommand::SetPendingSubscriptionItemsState { ids, state, reply } => {
                let result = crate::subscription_store::set_pending_items_state(
                    &mut self.connection,
                    &ids,
                    state,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            SubscriptionsCommand::ClearSubscriptionHistory { id, reply } => {
                let result =
                    crate::subscription_store::clear_history(&mut self.connection, id).await;
                publish_config(reply, result, &self.events);
            }
            SubscriptionsCommand::CompactSubscriptionItems { before, reply } => {
                let result = crate::subscription_store::compact_items(&mut self.connection, before)
                    .await
                    .map(|(removed, event)| {
                        if let Some(event) = event {
                            let _ = self.events.send(event);
                        }
                        removed
                    });
                send(reply, result);
            }
            SubscriptionsCommand::ArmSubscription {
                id,
                next_run_at,
                reply,
            } => {
                let result =
                    crate::subscription_store::arm(&mut self.connection, id, next_run_at).await;
                send(reply, result);
            }
            SubscriptionsCommand::FinishSubscriptionRun {
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
        }
    }
}
