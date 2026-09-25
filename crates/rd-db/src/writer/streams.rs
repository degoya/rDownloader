//! Live channels and their recordings: the writer half of `stream_store` and
//! `stream_schedule_store`.

use super::{Writer, publish_config, publish_unit_event};
use crate::commands::WriterCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_streams(&mut self, command: WriterCommand) {
        match command {
            WriterCommand::SetDownloadRecordingState { id, state, reply } => {
                let result = crate::stream_schedule_store::set_recording_state(
                    &mut self.connection,
                    id,
                    &state,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::CreateStreamSchedule { input, reply } => {
                let result =
                    crate::stream_schedule_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateStreamSchedule { id, input, reply } => {
                let result =
                    crate::stream_schedule_store::update(&mut self.connection, id, *input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteStreamSchedule { id, reply } => {
                let result = crate::stream_schedule_store::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::PlanStreamRuns {
                schedule_id,
                channel_id,
                occurrences,
                reply,
            } => {
                let result = crate::stream_schedule_store::plan_runs(
                    &mut self.connection,
                    schedule_id,
                    channel_id,
                    occurrences,
                )
                .await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::SetStreamRunState {
                id,
                state,
                download_id,
                replay_used,
                error,
                reply,
            } => {
                let result = crate::stream_schedule_store::set_run_state(
                    &mut self.connection,
                    id,
                    state,
                    download_id,
                    replay_used,
                    error,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::ExpireStreamRuns { cutoff, reply } => {
                let result =
                    crate::stream_schedule_store::expire_runs(&mut self.connection, cutoff).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::CreateStreamChannel { input, reply } => {
                let result = crate::stream_store::create(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::UpdateStreamChannel { id, input, reply } => {
                let result = crate::stream_store::update(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            WriterCommand::DeleteStreamChannel { id, reply } => {
                let result = crate::stream_store::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            WriterCommand::TouchStreamChannel {
                id,
                live_at,
                error,
                reply,
            } => {
                let result =
                    crate::stream_store::touch(&mut self.connection, id, live_at, error).await;
                publish_unit_event(reply, result, &self.events);
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
