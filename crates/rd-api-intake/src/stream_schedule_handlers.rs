//! REST surface for livestream schedules (RD-080-08).
//!
//! Validation happens here, against the same resolver the planner uses, so a schedule that
//! could never fire — an unknown zone, no weekdays, a window of zero — is refused while
//! somebody is looking at the form rather than discovered as a recording that never
//! happened.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_core::{
    ScheduleKind, StreamChannelId, StreamSchedule, StreamScheduleId, StreamScheduledRun,
};
use rd_db::NewStreamSchedule;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use crate::{AppState, error::ApiError};

const MAX_NAME: usize = 200;
/// Most runs one listing returns.
const RUN_LIMIT: i64 = 200;

/// Create or replace one schedule.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StreamScheduleRequest {
    pub channel_id: StreamChannelId,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(flatten)]
    pub kind: ScheduleKind,
    /// IANA zone, e.g. `Europe/Berlin`. An offset is refused: it does not survive daylight
    /// saving, which is the entire reason this field is not a number.
    pub timezone: String,
    pub window_minutes: u32,
    #[serde(default)]
    pub lead_minutes: u32,
    #[serde(default)]
    pub trail_minutes: u32,
    #[serde(default)]
    pub replay_from_start: bool,
}

const fn default_true() -> bool {
    true
}

/// Which schedule's runs to list.
#[derive(Debug, Deserialize, IntoParams)]
pub struct RunQuery {
    #[serde(default)]
    pub schedule_id: Option<StreamScheduleId>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Validates the request against the planner's own rules.
pub(crate) fn schedule_input(
    request: StreamScheduleRequest,
) -> Result<NewStreamSchedule, ApiError> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(
            "stream.schedule_name_invalid",
            "A schedule needs a name",
        ));
    }
    let input = NewStreamSchedule {
        channel_id: request.channel_id,
        name: name.to_owned(),
        enabled: request.enabled,
        kind: request.kind,
        timezone: request.timezone.trim().to_owned(),
        window_minutes: request.window_minutes,
        lead_minutes: request.lead_minutes,
        trail_minutes: request.trail_minutes,
        replay_from_start: request.replay_from_start,
    };
    // Checked with the resolver rather than with a copy of its rules, so the two cannot
    // disagree about what is valid.
    let probe = StreamSchedule {
        id: StreamScheduleId::new(),
        channel_id: input.channel_id,
        name: input.name.clone(),
        enabled: input.enabled,
        kind: input.kind.clone(),
        timezone: input.timezone.clone(),
        window_minutes: input.window_minutes,
        lead_minutes: input.lead_minutes,
        trail_minutes: input.trail_minutes,
        replay_from_start: input.replay_from_start,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    rd_stream::validate_schedule(&probe)
        .map_err(|error| ApiError::unprocessable(error.code(), error.message()))?;
    Ok(input)
}

#[utoipa::path(
    get,
    path = "/api/v1/streams/schedules",
    tag = "streams",
    responses((status = 200, body = Vec<StreamSchedule>))
)]
pub async fn list_stream_schedules(
    State(state): State<AppState>,
) -> Result<Json<Vec<StreamSchedule>>, ApiError> {
    Ok(Json(state.database.list_stream_schedules().await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/streams/schedules",
    tag = "streams",
    request_body = StreamScheduleRequest,
    responses(
        (status = 201, body = StreamSchedule),
        (status = 400, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn create_stream_schedule(
    State(state): State<AppState>,
    Json(request): Json<StreamScheduleRequest>,
) -> Result<(StatusCode, Json<StreamSchedule>), ApiError> {
    let input = schedule_input(request)?;
    let created = state.database.create_stream_schedule(input).await?;
    // Planned at once, so the upcoming occurrences are visible immediately rather than
    // after the monitor's next tick.
    state.stream_monitor.plan_now().await;
    Ok((StatusCode::CREATED, Json(created)))
}

#[utoipa::path(
    put,
    path = "/api/v1/streams/schedules/{id}",
    tag = "streams",
    params(("id" = StreamScheduleId, Path)),
    request_body = StreamScheduleRequest,
    responses(
        (status = 200, body = StreamSchedule),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn update_stream_schedule(
    State(state): State<AppState>,
    Path(id): Path<StreamScheduleId>,
    Json(request): Json<StreamScheduleRequest>,
) -> Result<Json<StreamSchedule>, ApiError> {
    let input = schedule_input(request)?;
    let updated = state
        .database
        .update_stream_schedule(id, input)
        .await
        .map_err(not_found)?;
    // The edit dropped the not-yet-started occurrences; this is what puts the new ones back.
    state.stream_monitor.plan_now().await;
    Ok(Json(updated))
}

#[utoipa::path(
    delete,
    path = "/api/v1/streams/schedules/{id}",
    tag = "streams",
    params(("id" = StreamScheduleId, Path)),
    responses((status = 204), (status = 404, body = crate::error::ErrorBody))
)]
pub async fn delete_stream_schedule(
    State(state): State<AppState>,
    Path(id): Path<StreamScheduleId>,
) -> Result<StatusCode, ApiError> {
    state
        .database
        .delete_stream_schedule(id)
        .await
        .map_err(not_found)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Planned, running, finished and missed occurrences.
///
/// A missed one is a row like any other rather than an absence, which is the point: a
/// recording that never happened is exactly what somebody needs to be able to see.
#[utoipa::path(
    get,
    path = "/api/v1/streams/runs",
    tag = "streams",
    params(RunQuery),
    responses((status = 200, body = Vec<StreamScheduledRun>))
)]
pub async fn list_stream_runs(
    State(state): State<AppState>,
    Query(query): Query<RunQuery>,
) -> Result<Json<Vec<StreamScheduledRun>>, ApiError> {
    let limit = query.limit.unwrap_or(RUN_LIMIT).clamp(1, RUN_LIMIT);
    Ok(Json(
        state
            .database
            .stream_scheduled_runs(query.schedule_id, limit)
            .await?,
    ))
}

fn not_found(error: anyhow::Error) -> ApiError {
    crate::error_codes::store_not_found(&error, "stream.schedule_not_found", "Schedule not found")
}
