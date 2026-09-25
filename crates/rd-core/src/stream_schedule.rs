//! Planned livestream recordings (RD-080-08).
//!
//! A schedule says *when* to watch a channel; the monitor then only probes inside that
//! window instead of round the clock. Two properties matter more than anything else here and
//! both are about time being harder than it looks:
//!
//! * **A local time stays a local time.** A weekly window at 20:00 Berlin is 20:00 Berlin in
//!   January and in July. Storing an offset would silently move the show by an hour twice a
//!   year, so the schedule stores an IANA zone and the occurrence is resolved through it.
//! * **An occurrence happens once.** Every planned run is keyed on its own start instant, so
//!   a restart, a duplicated tick, or the hour that repeats itself when the clocks go back
//!   cannot produce two recordings of one broadcast.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{DownloadId, StreamChannelId, StreamScheduleId, StreamScheduledRunId};

/// Minutes in a day, the exclusive upper bound for a start time.
pub const MINUTES_PER_DAY: u32 = 24 * 60;
/// Longest single recording window.
pub const MAX_WINDOW_MINUTES: u32 = 24 * 60;
/// Longest pre-roll or post-roll.
pub const MAX_ROLL_MINUTES: u32 = 120;
/// How far ahead occurrences are planned.
pub const PLANNING_HORIZON_DAYS: i64 = 14;

/// When a schedule fires.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ScheduleKind {
    /// A single known broadcast.
    Once {
        /// The instant it starts, in UTC. A one-off has an absolute time because it refers
        /// to one event, not to a time of day.
        start: DateTime<Utc>,
    },
    /// A recurring local time on chosen weekdays.
    Weekly {
        /// ISO weekdays, `1` = Monday through `7` = Sunday. Duplicates are ignored.
        days: Vec<u8>,
        /// Minutes after local midnight, `0..MINUTES_PER_DAY`.
        start_minute: u32,
    },
}

/// A planned recording of one channel.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StreamSchedule {
    pub id: StreamScheduleId,
    pub channel_id: StreamChannelId,
    pub name: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: ScheduleKind,
    /// IANA zone the local time is interpreted in, e.g. `Europe/Berlin`. Never an offset:
    /// an offset does not survive daylight saving.
    pub timezone: String,
    /// How long the window lasts, from its nominal start.
    pub window_minutes: u32,
    /// Extra minutes of watching before the window, for a broadcast that starts early.
    pub lead_minutes: u32,
    /// Extra minutes after it, for one that overruns.
    pub trail_minutes: u32,
    /// Ask the provider to start from the beginning of the stream, where it can.
    ///
    /// Only ever honoured when the provider actually offers it; see
    /// [`StreamScheduledRun::replay_used`].
    pub replay_from_start: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What became of one planned occurrence.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledRunState {
    /// Created, its window has not opened yet.
    #[default]
    Planned,
    /// The window is open and the channel is being watched.
    Waiting,
    /// A recording was started for it.
    Recording,
    /// The recording finished.
    Completed,
    /// The window closed without the channel ever going live.
    Missed,
    /// Something went wrong; `error` says what.
    Failed,
}

impl ScheduledRunState {
    /// Whether the run still needs attention from the monitor.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Planned | Self::Waiting | Self::Recording)
    }
}

/// One occurrence of a schedule.
///
/// Rows are created ahead of time rather than at the moment they fire, so a missed window is
/// visible as a row that ended in [`ScheduledRunState::Missed`] instead of being invisible
/// by never having existed.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct StreamScheduledRun {
    pub id: StreamScheduledRunId,
    pub schedule_id: StreamScheduleId,
    pub channel_id: StreamChannelId,
    /// The nominal start, which is also the idempotency key together with the schedule.
    pub starts_at: DateTime<Utc>,
    /// Nominal end, before the post-roll.
    pub ends_at: DateTime<Utc>,
    pub state: ScheduledRunState,
    /// The recording this run started, once it has one.
    pub download_id: Option<DownloadId>,
    /// Whether replay-from-start was actually available and used, as opposed to merely
    /// asked for. Recorded so the UI never claims a capability the provider did not offer.
    pub replay_used: bool,
    /// Redacted failure message.
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Why a schedule was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    UnknownTimezone,
    /// A weekly schedule with no weekdays fires never.
    NoDays,
    InvalidDay,
    StartOutOfRange,
    WindowOutOfRange,
    RollOutOfRange,
}

impl ScheduleError {
    /// Stable error code, following the `<domain>.<subject>_<condition>` convention.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownTimezone => "stream.schedule_timezone_unknown",
            Self::NoDays => "stream.schedule_no_days",
            Self::InvalidDay => "stream.schedule_day_invalid",
            Self::StartOutOfRange => "stream.schedule_start_invalid",
            Self::WindowOutOfRange => "stream.schedule_window_invalid",
            Self::RollOutOfRange => "stream.schedule_roll_invalid",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnknownTimezone => "That is not a known time zone",
            Self::NoDays => "A weekly schedule needs at least one weekday",
            Self::InvalidDay => "Weekdays run from 1 (Monday) to 7 (Sunday)",
            Self::StartOutOfRange => "The start time is not a time of day",
            Self::WindowOutOfRange => "The recording window is outside the permitted range",
            Self::RollOutOfRange => "The lead or trail time is outside the permitted range",
        }
    }
}
