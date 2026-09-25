//! Persistence of livestream schedules and their planned occurrences (RD-080-08).
//!
//! [`plan_runs`] is the interesting one. It inserts occurrences with
//! `ON CONFLICT DO NOTHING` against the UNIQUE index over `(schedule_id, starts_at)`, so
//! planning the same range twice — which happens on every restart and on every planner tick
//! — adds nothing the second time. Duplicate recordings of one broadcast are prevented by
//! the schema rather than by remembering to check.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{
    DownloadId, EventEnvelope, EventKind, ScheduleKind, ScheduledRunState, StreamChannelId,
    StreamSchedule, StreamScheduleId, StreamScheduledRun, StreamScheduledRunId,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, writer::insert_event};

/// Editable fields of a schedule; `create` assigns the id and timestamps.
#[derive(Clone, Debug)]
pub struct NewStreamSchedule {
    pub channel_id: StreamChannelId,
    pub name: String,
    pub enabled: bool,
    pub kind: ScheduleKind,
    pub timezone: String,
    pub window_minutes: u32,
    pub lead_minutes: u32,
    pub trail_minutes: u32,
    pub replay_from_start: bool,
}

/// One occurrence to plan.
#[derive(Clone, Copy, Debug)]
pub struct PlannedOccurrence {
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, channel_id, name, enabled, kind, start_at, days, start_minute, \
     timezone, window_minutes, lead_minutes, trail_minutes, replay_from_start, created_at, \
     updated_at";

const RUN_COLUMNS: &str = "id, schedule_id, channel_id, starts_at, ends_at, state, \
     download_id, replay_used, error, created_at";

fn changed_event() -> EventEnvelope {
    EventEnvelope::new(
        EventKind::StreamChanged,
        serde_json::json!({ "resource": "stream_schedule" }),
    )
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<StreamSchedule>> {
    sqlx::query_as::<_, ScheduleRow>(&format!(
        "SELECT {COLUMNS} FROM stream_schedules ORDER BY name, created_at"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn enabled(pool: &SqlitePool) -> Result<Vec<StreamSchedule>> {
    sqlx::query_as::<_, ScheduleRow>(&format!(
        "SELECT {COLUMNS} FROM stream_schedules WHERE enabled = 1"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn runs(
    pool: &SqlitePool,
    schedule_id: Option<StreamScheduleId>,
    limit: i64,
) -> Result<Vec<StreamScheduledRun>> {
    let rows =
        match schedule_id {
            Some(id) => {
                sqlx::query_as::<_, RunRow>(&format!(
                    "SELECT {RUN_COLUMNS} FROM stream_scheduled_runs WHERE schedule_id = ? \
                 ORDER BY starts_at DESC LIMIT ?"
                ))
                .bind(id.to_string())
                .bind(limit)
                .fetch_all(pool)
                .await?
            }
            None => sqlx::query_as::<_, RunRow>(&format!(
                "SELECT {RUN_COLUMNS} FROM stream_scheduled_runs ORDER BY starts_at DESC LIMIT ?"
            ))
            .bind(limit)
            .fetch_all(pool)
            .await?,
        };
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Runs that still need the monitor's attention, oldest first.
pub(crate) async fn open_runs(pool: &SqlitePool) -> Result<Vec<StreamScheduledRun>> {
    sqlx::query_as::<_, RunRow>(&format!(
        "SELECT {RUN_COLUMNS} FROM stream_scheduled_runs \
         WHERE state IN ('planned', 'waiting', 'recording') ORDER BY starts_at"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn create(
    connection: &mut SqliteConnection,
    input: NewStreamSchedule,
) -> Result<(StreamSchedule, EventEnvelope)> {
    let now = Utc::now();
    let value = StreamSchedule {
        id: StreamScheduleId::new(),
        channel_id: input.channel_id,
        name: input.name,
        enabled: input.enabled,
        kind: input.kind,
        timezone: input.timezone,
        window_minutes: input.window_minutes,
        lead_minutes: input.lead_minutes,
        trail_minutes: input.trail_minutes,
        replay_from_start: input.replay_from_start,
        created_at: now,
        updated_at: now,
    };
    let event = changed_event();
    let (kind, start_at, days, start_minute) = split_kind(&value.kind);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO stream_schedules (id, channel_id, name, enabled, kind, start_at, days, \
         start_minute, timezone, window_minutes, lead_minutes, trail_minutes, \
         replay_from_start, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(value.channel_id.to_string())
    .bind(&value.name)
    .bind(i64::from(value.enabled))
    .bind(kind)
    .bind(start_at)
    .bind(days)
    .bind(start_minute)
    .bind(&value.timezone)
    .bind(i64::from(value.window_minutes))
    .bind(i64::from(value.lead_minutes))
    .bind(i64::from(value.trail_minutes))
    .bind(i64::from(value.replay_from_start))
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn update(
    connection: &mut SqliteConnection,
    id: StreamScheduleId,
    input: NewStreamSchedule,
) -> Result<(StreamSchedule, EventEnvelope)> {
    let event = changed_event();
    let (kind, start_at, days, start_minute) = split_kind(&input.kind);
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE stream_schedules SET channel_id = ?, name = ?, enabled = ?, kind = ?, \
         start_at = ?, days = ?, start_minute = ?, timezone = ?, window_minutes = ?, \
         lead_minutes = ?, trail_minutes = ?, replay_from_start = ?, updated_at = ? \
         WHERE id = ?",
    )
    .bind(input.channel_id.to_string())
    .bind(&input.name)
    .bind(i64::from(input.enabled))
    .bind(kind)
    .bind(start_at)
    .bind(days)
    .bind(start_minute)
    .bind(&input.timezone)
    .bind(i64::from(input.window_minutes))
    .bind(i64::from(input.lead_minutes))
    .bind(i64::from(input.trail_minutes))
    .bind(i64::from(input.replay_from_start))
    .bind(Utc::now())
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::not_found("stream schedule not found"));
    }
    // Occurrences that have not started yet are re-planned from the new definition; ones
    // already recording or finished are history and are left alone.
    sqlx::query("DELETE FROM stream_scheduled_runs WHERE schedule_id = ? AND state = 'planned'")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let updated = sqlx::query_as::<_, ScheduleRow>(&format!(
        "SELECT {COLUMNS} FROM stream_schedules WHERE id = ?"
    ))
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((updated, event))
}

pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: StreamScheduleId,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    sqlx::query("DELETE FROM stream_scheduled_runs WHERE schedule_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("DELETE FROM stream_schedules WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::not_found("stream schedule not found"));
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Inserts occurrences that are not planned yet, returning how many were new.
///
/// The `ON CONFLICT DO NOTHING` is the whole point: planning runs on every tick and after
/// every restart, and re-planning a range that is already planned has to be free of effect.
pub(crate) async fn plan_runs(
    connection: &mut SqliteConnection,
    schedule_id: StreamScheduleId,
    channel_id: StreamChannelId,
    occurrences: Vec<PlannedOccurrence>,
) -> Result<(u32, EventEnvelope)> {
    let now = Utc::now();
    let event = changed_event();
    let mut created = 0_u32;
    let mut tx = connection.begin().await?;
    for occurrence in occurrences {
        let result = sqlx::query(
            "INSERT INTO stream_scheduled_runs (id, schedule_id, channel_id, starts_at, \
             ends_at, state, replay_used, created_at) \
             VALUES (?, ?, ?, ?, ?, 'planned', 0, ?) \
             ON CONFLICT(schedule_id, starts_at) DO NOTHING",
        )
        .bind(StreamScheduledRunId::new().to_string())
        .bind(schedule_id.to_string())
        .bind(channel_id.to_string())
        .bind(occurrence.starts_at)
        .bind(occurrence.ends_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        created += u32::try_from(result.rows_affected()).unwrap_or(0);
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((created, event))
}

/// Moves a run to a new state, optionally attaching the recording it started.
///
/// The state is only ever advanced from an *open* state, so a run that already completed
/// cannot be reopened by a late tick.
pub(crate) async fn set_run_state(
    connection: &mut SqliteConnection,
    id: StreamScheduledRunId,
    state: ScheduledRunState,
    download_id: Option<DownloadId>,
    replay_used: Option<bool>,
    error: Option<String>,
) -> Result<EventEnvelope> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE stream_scheduled_runs SET state = ?, \
         download_id = COALESCE(?, download_id), \
         replay_used = COALESCE(?, replay_used), \
         error = ? \
         WHERE id = ? AND state IN ('planned', 'waiting', 'recording')",
    )
    .bind(run_state_string(state))
    .bind(download_id.map(|id| id.to_string()))
    .bind(replay_used.map(i64::from))
    .bind(error)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        bail!("scheduled run is not open");
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Marks every open run whose window closed before `cutoff` as missed.
///
/// Run on every tick and once at startup: a window that passed while the service was down is
/// still a missed recording, and leaving it "planned" forever would hide that.
pub(crate) async fn expire_runs(
    connection: &mut SqliteConnection,
    cutoff: DateTime<Utc>,
) -> Result<(u32, EventEnvelope)> {
    let event = changed_event();
    let mut tx = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE stream_scheduled_runs SET state = 'missed' \
         WHERE state IN ('planned', 'waiting') AND ends_at < ?",
    )
    .bind(cutoff)
    .execute(&mut *tx)
    .await?;
    let expired = u32::try_from(result.rows_affected()).unwrap_or(0);
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((expired, event))
}

/// Stores a recording's segment history and sidecars (RD-080-09).
///
/// Written after every segment, so a crash mid-recording leaves a history that says what is
/// on disk rather than nothing at all.
pub(crate) async fn set_recording_state(
    connection: &mut SqliteConnection,
    id: DownloadId,
    state: &rd_core::RecordingState,
) -> Result<EventEnvelope> {
    let event = EventEnvelope::new(
        EventKind::DownloadProgress,
        serde_json::json!({ "download_id": id }),
    );
    let mut tx = connection.begin().await?;
    sqlx::query("UPDATE downloads SET recording_json = ? WHERE id = ?")
        .bind(serde_json::to_string(state)?)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event)
}

fn split_kind(
    kind: &ScheduleKind,
) -> (
    &'static str,
    Option<DateTime<Utc>>,
    Option<String>,
    Option<i64>,
) {
    match kind {
        ScheduleKind::Once { start } => ("once", Some(*start), None, None),
        ScheduleKind::Weekly { days, start_minute } => (
            "weekly",
            None,
            Some(
                days.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            Some(i64::from(*start_minute)),
        ),
    }
}

const fn run_state_string(state: ScheduledRunState) -> &'static str {
    match state {
        ScheduledRunState::Planned => "planned",
        ScheduledRunState::Waiting => "waiting",
        ScheduledRunState::Recording => "recording",
        ScheduledRunState::Completed => "completed",
        ScheduledRunState::Missed => "missed",
        ScheduledRunState::Failed => "failed",
    }
}

#[derive(FromRow)]
struct ScheduleRow {
    id: String,
    channel_id: String,
    name: String,
    enabled: i64,
    kind: String,
    start_at: Option<DateTime<Utc>>,
    days: Option<String>,
    start_minute: Option<i64>,
    timezone: String,
    window_minutes: i64,
    lead_minutes: i64,
    trail_minutes: i64,
    replay_from_start: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<ScheduleRow> for StreamSchedule {
    type Error = anyhow::Error;

    fn try_from(row: ScheduleRow) -> Result<Self> {
        let kind = if row.kind == "once" {
            ScheduleKind::Once {
                start: row.start_at.context("one-off schedule has no start")?,
            }
        } else {
            ScheduleKind::Weekly {
                days: row
                    .days
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|value| value.trim().parse().ok())
                    .collect(),
                start_minute: row
                    .start_minute
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or_default(),
            }
        };
        Ok(Self {
            id: StreamScheduleId::from_uuid(row.id.parse()?),
            channel_id: StreamChannelId::from_uuid(row.channel_id.parse()?),
            name: row.name,
            enabled: row.enabled != 0,
            kind,
            timezone: row.timezone,
            window_minutes: u32::try_from(row.window_minutes).unwrap_or_default(),
            lead_minutes: u32::try_from(row.lead_minutes).unwrap_or_default(),
            trail_minutes: u32::try_from(row.trail_minutes).unwrap_or_default(),
            replay_from_start: row.replay_from_start != 0,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
struct RunRow {
    id: String,
    schedule_id: String,
    channel_id: String,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    state: String,
    download_id: Option<String>,
    replay_used: i64,
    error: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<RunRow> for StreamScheduledRun {
    type Error = anyhow::Error;

    fn try_from(row: RunRow) -> Result<Self> {
        Ok(Self {
            id: StreamScheduledRunId::from_uuid(row.id.parse()?),
            schedule_id: StreamScheduleId::from_uuid(row.schedule_id.parse()?),
            channel_id: StreamChannelId::from_uuid(row.channel_id.parse()?),
            starts_at: row.starts_at,
            ends_at: row.ends_at,
            state: match row.state.as_str() {
                "waiting" => ScheduledRunState::Waiting,
                "recording" => ScheduledRunState::Recording,
                "completed" => ScheduledRunState::Completed,
                "missed" => ScheduledRunState::Missed,
                "failed" => ScheduledRunState::Failed,
                _ => ScheduledRunState::Planned,
            },
            download_id: row
                .download_id
                .map(|value| value.parse().map(DownloadId::from_uuid))
                .transpose()?,
            replay_used: row.replay_used != 0,
            error: row.error,
            created_at: row.created_at,
        })
    }
}
