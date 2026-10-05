//! Live channels and their recordings: the writer half of `stream_store` and
//! `stream_schedule_store`.

use super::{Writer, publish_config, publish_unit_event};
use crate::commands::StreamsCommand;

impl Writer {
    /// Applies the commands this module owns; see the module documentation for which.
    pub(super) async fn handle_streams(&mut self, command: StreamsCommand) {
        match command {
            StreamsCommand::SetDownloadRecordingState { id, state, reply } => {
                let result = crate::stream_schedule_store::set_recording_state(
                    &mut self.connection,
                    id,
                    &state,
                )
                .await;
                publish_unit_event(reply, result, &self.events);
            }
            StreamsCommand::CreateStreamSchedule { input, reply } => {
                let result =
                    crate::stream_schedule_store::create(&mut self.connection, *input).await;
                publish_config(reply, result, &self.events);
            }
            StreamsCommand::UpdateStreamSchedule { id, input, reply } => {
                let result =
                    crate::stream_schedule_store::update(&mut self.connection, id, *input).await;
                publish_config(reply, result, &self.events);
            }
            StreamsCommand::DeleteStreamSchedule { id, reply } => {
                let result = crate::stream_schedule_store::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            StreamsCommand::PlanStreamRuns {
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
            StreamsCommand::SetStreamRunState {
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
            StreamsCommand::ExpireStreamRuns { cutoff, reply } => {
                let result =
                    crate::stream_schedule_store::expire_runs(&mut self.connection, cutoff).await;
                publish_config(reply, result, &self.events);
            }
            StreamsCommand::CreateStreamChannel { input, reply } => {
                let result = crate::stream_store::create(&mut self.connection, input).await;
                publish_config(reply, result, &self.events);
            }
            StreamsCommand::UpdateStreamChannel { id, input, reply } => {
                let result = crate::stream_store::update(&mut self.connection, id, input).await;
                publish_config(reply, result, &self.events);
            }
            StreamsCommand::DeleteStreamChannel { id, reply } => {
                let result = crate::stream_store::delete(&mut self.connection, id).await;
                publish_unit_event(reply, result, &self.events);
            }
            StreamsCommand::TouchStreamChannel {
                id,
                live_at,
                error,
                reply,
            } => {
                let result =
                    crate::stream_store::touch(&mut self.connection, id, live_at, error).await;
                publish_unit_event(reply, result, &self.events);
            }
        }
    }
}
