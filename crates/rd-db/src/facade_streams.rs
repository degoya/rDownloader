//! Database facade for stream channels, recording state and livestream schedules (RD-080-08,
//! RD-080-09).

use anyhow::Result;

use crate::{Database, commands::StreamsCommand, writer};

impl Database {
    /// Lists the livestream channels watched by the recording monitor.
    pub async fn list_stream_channels(&self) -> Result<Vec<rd_core::StreamChannel>> {
        crate::stream_store::list(&self.readers).await
    }

    pub async fn create_stream_channel(
        &self,
        input: crate::stream_store::NewStreamChannel,
    ) -> Result<rd_core::StreamChannel> {
        writer::request(&self.writer, |reply| StreamsCommand::CreateStreamChannel {
            input,
            reply,
        })
        .await
    }

    pub async fn update_stream_channel(
        &self,
        id: rd_core::StreamChannelId,
        input: crate::stream_store::NewStreamChannel,
    ) -> Result<rd_core::StreamChannel> {
        writer::request(&self.writer, |reply| StreamsCommand::UpdateStreamChannel {
            id,
            input,
            reply,
        })
        .await
    }

    pub async fn delete_stream_channel(&self, id: rd_core::StreamChannelId) -> Result<()> {
        writer::request(&self.writer, |reply| StreamsCommand::DeleteStreamChannel {
            id,
            reply,
        })
        .await
    }

    /// Records a monitor probe outcome (live timestamp and/or latest error).
    pub async fn touch_stream_channel(
        &self,
        id: rd_core::StreamChannelId,
        live_at: Option<chrono::DateTime<chrono::Utc>>,
        error: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| StreamsCommand::TouchStreamChannel {
            id,
            live_at,
            error,
            reply,
        })
        .await
    }

    /// Stores a recording's segment history and sidecars (RD-080-09).
    pub async fn set_download_recording_state(
        &self,
        id: rd_core::DownloadId,
        state: rd_core::RecordingState,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::StreamsCommand::SetDownloadRecordingState {
                id,
                state: Box::new(state),
                reply,
            }
        })
        .await
    }

    // ---- Livestream schedules (RD-080-08) ----

    pub async fn list_stream_schedules(&self) -> Result<Vec<rd_core::StreamSchedule>> {
        crate::stream_schedule_store::list(&self.readers).await
    }

    pub async fn enabled_stream_schedules(&self) -> Result<Vec<rd_core::StreamSchedule>> {
        crate::stream_schedule_store::enabled(&self.readers).await
    }

    pub async fn stream_scheduled_runs(
        &self,
        schedule_id: Option<rd_core::StreamScheduleId>,
        limit: i64,
    ) -> Result<Vec<rd_core::StreamScheduledRun>> {
        crate::stream_schedule_store::runs(&self.readers, schedule_id, limit).await
    }

    /// Runs that still need the monitor's attention.
    pub async fn open_stream_runs(&self) -> Result<Vec<rd_core::StreamScheduledRun>> {
        crate::stream_schedule_store::open_runs(&self.readers).await
    }

    pub async fn create_stream_schedule(
        &self,
        input: crate::stream_schedule_store::NewStreamSchedule,
    ) -> Result<rd_core::StreamSchedule> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::StreamsCommand::CreateStreamSchedule {
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    pub async fn update_stream_schedule(
        &self,
        id: rd_core::StreamScheduleId,
        input: crate::stream_schedule_store::NewStreamSchedule,
    ) -> Result<rd_core::StreamSchedule> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::StreamsCommand::UpdateStreamSchedule {
                id,
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    pub async fn delete_stream_schedule(&self, id: rd_core::StreamScheduleId) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::StreamsCommand::DeleteStreamSchedule { id, reply }
        })
        .await
    }

    /// Plans occurrences, skipping any already planned. Returns how many were new.
    pub async fn plan_stream_runs(
        &self,
        schedule_id: rd_core::StreamScheduleId,
        channel_id: rd_core::StreamChannelId,
        occurrences: Vec<crate::stream_schedule_store::PlannedOccurrence>,
    ) -> Result<u32> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::StreamsCommand::PlanStreamRuns {
                schedule_id,
                channel_id,
                occurrences,
                reply,
            }
        })
        .await
    }

    pub async fn set_stream_run_state(
        &self,
        id: rd_core::StreamScheduledRunId,
        state: rd_core::ScheduledRunState,
        download_id: Option<rd_core::DownloadId>,
        replay_used: Option<bool>,
        error: Option<String>,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::StreamsCommand::SetStreamRunState {
                id,
                state,
                download_id,
                replay_used,
                error,
                reply,
            }
        })
        .await
    }

    /// Marks open runs whose window closed before `cutoff` as missed.
    pub async fn expire_stream_runs(&self, cutoff: chrono::DateTime<chrono::Utc>) -> Result<u32> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::StreamsCommand::ExpireStreamRuns { cutoff, reply }
        })
        .await
    }
}
