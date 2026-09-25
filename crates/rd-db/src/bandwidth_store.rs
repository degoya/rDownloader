//! Persistence of bandwidth profiles, their weekly schedule and the traffic counters.

use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{BandwidthProfileId, BandwidthWindowId, EventEnvelope, EventKind};
use rd_limits::{
    BandwidthProfile, BudgetPeriod, BudgetState, BudgetStates, DaySet, ScheduleWindow, ScopeLimit,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, parse_id, writer::insert_event};

/// Editable fields of a profile; the id stays with the row.
#[derive(Clone, Debug)]
pub struct NewBandwidthProfile {
    pub name: String,
    pub download_bytes_per_second: Option<rd_core::ByteCount>,
    pub upload_bytes_per_second: Option<rd_core::ByteCount>,
    pub max_active_files: Option<u32>,
    pub daily_budget_bytes: Option<rd_core::ByteCount>,
    pub monthly_budget_bytes: Option<rd_core::ByteCount>,
    pub scopes: Vec<ScopeLimit>,
}

/// One window of the weekly schedule, without an id — the schedule is replaced as a whole.
#[derive(Clone, Debug)]
pub struct NewScheduleWindow {
    pub profile_id: BandwidthProfileId,
    pub days: DaySet,
    pub start_minute: u16,
    pub end_minute: u16,
    pub priority: i32,
    pub enabled: bool,
}

#[derive(FromRow)]
struct ProfileRow {
    id: String,
    name: String,
    download_bytes_per_second: Option<i64>,
    upload_bytes_per_second: Option<i64>,
    max_active_files: Option<i64>,
    daily_budget_bytes: Option<i64>,
    monthly_budget_bytes: Option<i64>,
    scopes_json: String,
}

impl TryFrom<ProfileRow> for BandwidthProfile {
    type Error = anyhow::Error;

    fn try_from(row: ProfileRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            download_bytes_per_second: optional_bytes(row.download_bytes_per_second)?,
            upload_bytes_per_second: optional_bytes(row.upload_bytes_per_second)?,
            max_active_files: row
                .max_active_files
                .and_then(|value| u32::try_from(value).ok()),
            daily_budget_bytes: optional_bytes(row.daily_budget_bytes)?,
            monthly_budget_bytes: optional_bytes(row.monthly_budget_bytes)?,
            scopes: serde_json::from_str(&row.scopes_json).context("stored scope limits")?,
        })
    }
}

#[derive(FromRow)]
struct WindowRow {
    id: String,
    profile_id: String,
    days: i64,
    start_minute: i64,
    end_minute: i64,
    priority: i64,
    enabled: bool,
}

impl TryFrom<WindowRow> for ScheduleWindow {
    type Error = anyhow::Error;

    fn try_from(row: WindowRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            profile_id: parse_id(&row.profile_id)?,
            days: DaySet(u8::try_from(row.days).context("day mask out of range")?),
            start_minute: u16::try_from(row.start_minute).context("start minute out of range")?,
            end_minute: u16::try_from(row.end_minute).context("end minute out of range")?,
            priority: i32::try_from(row.priority).context("priority out of range")?,
            enabled: row.enabled,
        })
    }
}

const PROFILE_COLUMNS: &str = "id, name, download_bytes_per_second, upload_bytes_per_second, \
     max_active_files, daily_budget_bytes, monthly_budget_bytes, scopes_json";

pub(crate) async fn list_profiles(pool: &SqlitePool) -> Result<Vec<BandwidthProfile>> {
    sqlx::query_as::<_, ProfileRow>(&format!(
        "SELECT {PROFILE_COLUMNS} FROM bandwidth_profiles ORDER BY name"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn list_windows(pool: &SqlitePool) -> Result<Vec<ScheduleWindow>> {
    sqlx::query_as::<_, WindowRow>(
        "SELECT id, profile_id, days, start_minute, end_minute, priority, enabled \
         FROM bandwidth_windows ORDER BY start_minute, priority",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn create_profile(
    connection: &mut SqliteConnection,
    input: NewBandwidthProfile,
) -> Result<(BandwidthProfile, EventEnvelope)> {
    let value = BandwidthProfile {
        id: BandwidthProfileId::new(),
        name: input.name,
        download_bytes_per_second: input.download_bytes_per_second,
        upload_bytes_per_second: input.upload_bytes_per_second,
        max_active_files: input.max_active_files,
        daily_budget_bytes: input.daily_budget_bytes,
        monthly_budget_bytes: input.monthly_budget_bytes,
        scopes: input.scopes,
    };
    let now = Utc::now();
    let event = changed_event(value.id);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "INSERT INTO bandwidth_profiles (id, name, download_bytes_per_second, \
         upload_bytes_per_second, max_active_files, daily_budget_bytes, monthly_budget_bytes, \
         scopes_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(value.id.to_string())
    .bind(&value.name)
    .bind(value.download_bytes_per_second.map(persisted))
    .bind(value.upload_bytes_per_second.map(persisted))
    .bind(value.max_active_files.map(i64::from))
    .bind(value.daily_budget_bytes.map(persisted))
    .bind(value.monthly_budget_bytes.map(persisted))
    .bind(serde_json::to_string(&value.scopes)?)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn update_profile(
    connection: &mut SqliteConnection,
    id: BandwidthProfileId,
    input: NewBandwidthProfile,
) -> Result<(BandwidthProfile, EventEnvelope)> {
    let value = BandwidthProfile {
        id,
        name: input.name,
        download_bytes_per_second: input.download_bytes_per_second,
        upload_bytes_per_second: input.upload_bytes_per_second,
        max_active_files: input.max_active_files,
        daily_budget_bytes: input.daily_budget_bytes,
        monthly_budget_bytes: input.monthly_budget_bytes,
        scopes: input.scopes,
    };
    let now = Utc::now();
    let event = changed_event(id);
    let mut tx = connection.begin().await?;
    let updated = sqlx::query(
        "UPDATE bandwidth_profiles SET name = ?, download_bytes_per_second = ?, \
         upload_bytes_per_second = ?, max_active_files = ?, daily_budget_bytes = ?, \
         monthly_budget_bytes = ?, scopes_json = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&value.name)
    .bind(value.download_bytes_per_second.map(persisted))
    .bind(value.upload_bytes_per_second.map(persisted))
    .bind(value.max_active_files.map(i64::from))
    .bind(value.daily_budget_bytes.map(persisted))
    .bind(value.monthly_budget_bytes.map(persisted))
    .bind(serde_json::to_string(&value.scopes)?)
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        updated.rows_affected() > 0,
        StoreError::not_found("bandwidth profile not found")
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((value, event))
}

pub(crate) async fn delete_profile(
    connection: &mut SqliteConnection,
    id: BandwidthProfileId,
) -> Result<EventEnvelope> {
    let event = changed_event(id);
    let mut tx = connection.begin().await?;
    // The windows go with the profile; a window without one could never activate.
    sqlx::query("DELETE FROM bandwidth_windows WHERE profile_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM bandwidth_budget_counters WHERE profile_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    let deleted = sqlx::query("DELETE FROM bandwidth_profiles WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    anyhow::ensure!(
        deleted.rows_affected() > 0,
        StoreError::not_found("bandwidth profile not found")
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Replaces the whole weekly schedule.
///
/// A weekly plan is edited as one document, so replacing it atomically avoids the
/// half-applied states a per-window CRUD would allow.
pub(crate) async fn replace_windows(
    connection: &mut SqliteConnection,
    windows: Vec<NewScheduleWindow>,
) -> Result<(Vec<ScheduleWindow>, EventEnvelope)> {
    let now = Utc::now();
    let event = EventEnvelope::new(
        EventKind::BandwidthChanged,
        serde_json::json!({ "entity": "schedule" }),
    );
    let mut tx = connection.begin().await?;
    sqlx::query("DELETE FROM bandwidth_windows")
        .execute(&mut *tx)
        .await?;
    let mut stored = Vec::with_capacity(windows.len());
    for window in windows {
        let value = ScheduleWindow {
            id: BandwidthWindowId::new(),
            profile_id: window.profile_id,
            days: window.days,
            start_minute: window.start_minute,
            end_minute: window.end_minute,
            priority: window.priority,
            enabled: window.enabled,
        };
        sqlx::query(
            "INSERT INTO bandwidth_windows (id, profile_id, days, start_minute, end_minute, \
             priority, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(value.id.to_string())
        .bind(value.profile_id.to_string())
        .bind(i64::from(value.days.0))
        .bind(i64::from(value.start_minute))
        .bind(i64::from(value.end_minute))
        .bind(i64::from(value.priority))
        .bind(value.enabled)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        stored.push(value);
    }
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok((stored, event))
}

#[derive(FromRow)]
struct CounterRow {
    profile_id: String,
    period_kind: String,
    period_key: String,
    used_bytes: i64,
}

pub(crate) async fn budget_states(pool: &SqlitePool) -> Result<BudgetStates> {
    let rows = sqlx::query_as::<_, CounterRow>(
        "SELECT profile_id, period_kind, period_key, used_bytes FROM bandwidth_budget_counters",
    )
    .fetch_all(pool)
    .await?;
    let mut states = BudgetStates::new();
    for row in rows {
        let used_bytes = u64::try_from(row.used_bytes).unwrap_or_default();
        let period = BudgetPeriod {
            key: row.period_key,
            used_bytes,
        };
        let state = states.entry(row.profile_id).or_default();
        if row.period_kind == "monthly" {
            state.monthly = period;
        } else {
            state.daily = period;
        }
    }
    Ok(states)
}

pub(crate) async fn store_budget_state(
    connection: &mut SqliteConnection,
    profile_id: BandwidthProfileId,
    state: BudgetState,
) -> Result<()> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    for (kind, period) in [("daily", &state.daily), ("monthly", &state.monthly)] {
        sqlx::query(
            "INSERT INTO bandwidth_budget_counters (profile_id, period_kind, period_key, \
             used_bytes, updated_at) VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(profile_id, period_kind) DO UPDATE SET period_key = excluded.period_key, \
             used_bytes = excluded.used_bytes, updated_at = excluded.updated_at",
        )
        .bind(profile_id.to_string())
        .bind(kind)
        .bind(&period.key)
        .bind(i64::try_from(period.used_bytes).unwrap_or(i64::MAX))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn changed_event(id: BandwidthProfileId) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::BandwidthChanged,
        serde_json::json!({ "entity": "profile", "id": id }),
    )
}

/// A `ByteCount` is bounded by SQLite's INTEGER range at construction, so this is lossless.
fn persisted(value: rd_core::ByteCount) -> i64 {
    i64::try_from(value.get()).unwrap_or(i64::MAX)
}

fn optional_bytes(value: Option<i64>) -> Result<Option<rd_core::ByteCount>> {
    value
        .map(|value| {
            let value = u64::try_from(value).context("negative byte count")?;
            rd_core::ByteCount::new(value).map_err(anyhow::Error::msg)
        })
        .transpose()
}
