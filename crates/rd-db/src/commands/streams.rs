//! The commands of `writer/streams.rs`.

use super::Reply;

/// The commands `Writer::handle_streams` applies.
pub(crate) enum StreamsCommand {
    /// Segment history and sidecars of a recording (RD-080-09).
    SetDownloadRecordingState {
        id: rd_core::DownloadId,
        state: Box<rd_core::RecordingState>,
        reply: Reply<()>,
    },
    /// Livestream schedules (RD-080-08).
    CreateStreamSchedule {
        input: Box<crate::stream_schedule_store::NewStreamSchedule>,
        reply: Reply<rd_core::StreamSchedule>,
    },
    UpdateStreamSchedule {
        id: rd_core::StreamScheduleId,
        input: Box<crate::stream_schedule_store::NewStreamSchedule>,
        reply: Reply<rd_core::StreamSchedule>,
    },
    DeleteStreamSchedule {
        id: rd_core::StreamScheduleId,
        reply: Reply<()>,
    },
    /// Inserts occurrences that are not planned yet; returns how many were new.
    PlanStreamRuns {
        schedule_id: rd_core::StreamScheduleId,
        channel_id: rd_core::StreamChannelId,
        occurrences: Vec<crate::stream_schedule_store::PlannedOccurrence>,
        reply: Reply<u32>,
    },
    SetStreamRunState {
        id: rd_core::StreamScheduledRunId,
        state: rd_core::ScheduledRunState,
        download_id: Option<rd_core::DownloadId>,
        replay_used: Option<bool>,
        error: Option<String>,
        reply: Reply<()>,
    },
    /// Marks open runs whose window has closed as missed.
    ExpireStreamRuns {
        cutoff: chrono::DateTime<chrono::Utc>,
        reply: Reply<u32>,
    },
    CreateStreamChannel {
        input: crate::stream_store::NewStreamChannel,
        reply: Reply<rd_core::StreamChannel>,
    },
    UpdateStreamChannel {
        id: rd_core::StreamChannelId,
        input: crate::stream_store::NewStreamChannel,
        reply: Reply<rd_core::StreamChannel>,
    },
    DeleteStreamChannel {
        id: rd_core::StreamChannelId,
        reply: Reply<()>,
    },
    TouchStreamChannel {
        id: rd_core::StreamChannelId,
        live_at: Option<chrono::DateTime<chrono::Utc>>,
        error: Option<String>,
        reply: Reply<()>,
    },
}
