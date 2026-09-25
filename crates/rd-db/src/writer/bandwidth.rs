//! The writer half of `bandwidth_store`: profiles, their windows and their budget counters.

use super::{Writer, publish_config, publish_unit_event, send};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_bandwidth(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::CreateBandwidthProfile { input, reply } => {
                let result =
                    crate::bandwidth_store::create_profile(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateBandwidthProfile { id, input, reply } => {
                let result =
                    crate::bandwidth_store::update_profile(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteBandwidthProfile { id, reply } => {
                let result = crate::bandwidth_store::delete_profile(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::ReplaceBandwidthWindows { windows, reply } => {
                let result =
                    crate::bandwidth_store::replace_windows(&mut self.connection, windows).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::StoreBandwidthBudget {
                profile_id,
                state,
                reply,
            } => {
                // Counters change every few seconds; broadcasting each tick would be
                // noise, so this write stays silent.
                let result = crate::bandwidth_store::store_budget_state(
                    &mut self.connection,
                    profile_id,
                    state,
                )
                .await;
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
