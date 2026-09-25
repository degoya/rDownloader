//! Bandwidth profiles, the weekly schedule, the live status and the capability matrix
//! (RD-050-12).

use axum::{Json, extract::Path as AxumPath, extract::State};
use rd_limits::{DaySet, LimitSource, MINUTES_PER_DAY, RunnerLimitSupport, ScopeLimit};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// Highest weekly windows accepted, so one request cannot blow up the schedule evaluation.
const MAX_WINDOWS: usize = 200;
/// Highest scope limits per profile.
const MAX_SCOPES: usize = 100;

#[derive(Deserialize, ToSchema)]
pub struct BandwidthProfileRequest {
    pub name: String,
    /// Global download limit; empty = unlimited.
    #[serde(default)]
    pub download_bytes_per_second: Option<rd_core::ByteCount>,
    /// Global torrent upload limit; empty = unlimited.
    #[serde(default)]
    pub upload_bytes_per_second: Option<rd_core::ByteCount>,
    /// Caps the queue's parallelism while the profile is active; empty keeps the setting.
    #[serde(default)]
    pub max_active_files: Option<u32>,
    #[serde(default)]
    pub daily_budget_bytes: Option<rd_core::ByteCount>,
    #[serde(default)]
    pub monthly_budget_bytes: Option<rd_core::ByteCount>,
    #[serde(default)]
    pub scopes: Vec<ScopeLimit>,
}

#[derive(Deserialize, ToSchema)]
pub struct ScheduleWindowRequest {
    pub profile_id: rd_core::BandwidthProfileId,
    /// Monday-first bitmask; bit 0 = Monday.
    pub days: u8,
    pub start_minute: u16,
    /// Exclusive; below `start_minute` the window wraps past midnight.
    pub end_minute: u16,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_enabled() -> bool {
    true
}

#[derive(Deserialize, ToSchema)]
pub struct ScheduleRequest {
    /// IANA timezone the windows and budget periods are read in.
    pub timezone: String,
    /// Profile used outside every window; empty = no limits.
    #[serde(default)]
    pub default_profile_id: Option<rd_core::BandwidthProfileId>,
    pub windows: Vec<ScheduleWindowRequest>,
}

#[derive(Serialize, ToSchema)]
pub struct ScheduleResponse {
    pub timezone: String,
    pub default_profile_id: Option<rd_core::BandwidthProfileId>,
    pub windows: Vec<rd_limits::ScheduleWindow>,
}

#[derive(Serialize, ToSchema)]
pub struct BudgetUsageResponse {
    pub period_key: String,
    pub used_bytes: rd_core::ByteCount,
    pub limit_bytes: Option<rd_core::ByteCount>,
}

#[derive(Serialize, ToSchema)]
pub struct BindingLimitResponse {
    pub bytes_per_second: rd_core::ByteCount,
    pub source: LimitSource,
}

#[derive(Serialize, ToSchema)]
pub struct BandwidthStatusResponse {
    pub timezone: String,
    pub active_profile: Option<rd_limits::BandwidthProfile>,
    pub next_switch_at: Option<chrono::DateTime<chrono::Utc>>,
    pub daily: Option<BudgetUsageResponse>,
    pub monthly: Option<BudgetUsageResponse>,
    /// Set while the budget holds back new transfers; running ones finish.
    pub budget_exhausted: bool,
    /// The strictest limit a plain HTTP download would meet right now.
    pub binding_limit: Option<BindingLimitResponse>,
}

#[utoipa::path(get, path = "/api/v1/bandwidth/profiles", tag = "bandwidth", responses((status = 200, body = [rd_limits::BandwidthProfile])))]
pub async fn list_profiles(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_limits::BandwidthProfile>>, ApiError> {
    Ok(Json(state.database.list_bandwidth_profiles().await?))
}

#[utoipa::path(post, path = "/api/v1/bandwidth/profiles", tag = "bandwidth", request_body = BandwidthProfileRequest, responses((status = 201, body = rd_limits::BandwidthProfile)))]
pub async fn create_profile(
    State(state): State<AppState>,
    Json(request): Json<BandwidthProfileRequest>,
) -> Result<(axum::http::StatusCode, Json<rd_limits::BandwidthProfile>), ApiError> {
    let input = validated_profile(request)?;
    let profile = state.database.create_bandwidth_profile(input).await?;
    state.scheduler.reload_bandwidth().await?;
    Ok((axum::http::StatusCode::CREATED, Json(profile)))
}

#[utoipa::path(put, path = "/api/v1/bandwidth/profiles/{id}", tag = "bandwidth", params(("id" = rd_core::BandwidthProfileId, Path)), request_body = BandwidthProfileRequest, responses((status = 200, body = rd_limits::BandwidthProfile), (status = 404)))]
pub async fn update_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::BandwidthProfileId>,
    Json(request): Json<BandwidthProfileRequest>,
) -> Result<Json<rd_limits::BandwidthProfile>, ApiError> {
    let input = validated_profile(request)?;
    let profile = state
        .database
        .update_bandwidth_profile(id, input)
        .await
        .map_err(not_found)?;
    state.scheduler.reload_bandwidth().await?;
    Ok(Json(profile))
}

#[utoipa::path(delete, path = "/api/v1/bandwidth/profiles/{id}", tag = "bandwidth", params(("id" = rd_core::BandwidthProfileId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn delete_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::BandwidthProfileId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state
        .database
        .delete_bandwidth_profile(id)
        .await
        .map_err(not_found)?;
    state.scheduler.reload_bandwidth().await?;
    Ok(Json(crate::dto::MessageResponse::new(
        "bandwidth.profile_deleted",
        "Bandwidth profile deleted",
    )))
}

#[utoipa::path(get, path = "/api/v1/bandwidth/schedule", tag = "bandwidth", responses((status = 200, body = ScheduleResponse)))]
pub async fn get_schedule(
    State(state): State<AppState>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    let settings = crate::handlers::read_settings(&state).await?;
    Ok(Json(ScheduleResponse {
        timezone: settings.bandwidth_timezone,
        default_profile_id: settings.bandwidth_default_profile_id,
        windows: state.database.list_bandwidth_windows().await?,
    }))
}

/// Replaces the weekly schedule as one document — a plan is edited as a whole, and a
/// partial apply would leave a schedule nobody asked for active in between.
#[utoipa::path(put, path = "/api/v1/bandwidth/schedule", tag = "bandwidth", request_body = ScheduleRequest, responses((status = 200, body = ScheduleResponse), (status = 400, body = crate::error::ErrorBody)))]
pub async fn put_schedule(
    State(state): State<AppState>,
    Json(request): Json<ScheduleRequest>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    rd_limits::parse_timezone(&request.timezone).map_err(|_| {
        ApiError::bad_request("bandwidth.timezone_invalid", "Unknown timezone")
            .with_param("timezone", &request.timezone)
    })?;
    if request.windows.len() > MAX_WINDOWS {
        return Err(ApiError::bad_request(
            "bandwidth.too_many_windows",
            "The schedule has too many windows",
        )
        .with_param("max", MAX_WINDOWS));
    }
    let profiles = state.database.list_bandwidth_profiles().await?;
    let known = |id: rd_core::BandwidthProfileId| profiles.iter().any(|profile| profile.id == id);
    if let Some(default) = request.default_profile_id
        && !known(default)
    {
        return Err(profile_not_found());
    }
    let mut windows = Vec::with_capacity(request.windows.len());
    for window in request.windows {
        if !known(window.profile_id) {
            return Err(profile_not_found());
        }
        if window.days & DaySet::EVERY_DAY.0 == 0 {
            return Err(ApiError::bad_request(
                "bandwidth.window_invalid",
                "A window must apply to at least one weekday",
            ));
        }
        if window.start_minute >= MINUTES_PER_DAY
            || window.end_minute > MINUTES_PER_DAY
            || window.start_minute == window.end_minute
        {
            return Err(ApiError::bad_request(
                "bandwidth.window_invalid",
                "A window must span a non-empty part of the day",
            ));
        }
        windows.push(rd_db::NewScheduleWindow {
            profile_id: window.profile_id,
            days: DaySet(window.days & DaySet::EVERY_DAY.0),
            start_minute: window.start_minute,
            end_minute: window.end_minute,
            priority: window.priority,
            enabled: window.enabled,
        });
    }
    let stored = state.database.replace_bandwidth_windows(windows).await?;
    // The two schedule keys live in the shared settings blob so they ride the settings
    // backup, but they are written directly: re-applying the whole document would also
    // re-apply the login, media, torrent and post-processing settings, which a schedule
    // edit has no business touching.
    let mut settings = crate::handlers::read_settings(&state).await?;
    settings.bandwidth_timezone = request.timezone.clone();
    settings.bandwidth_default_profile_id = request.default_profile_id;
    state
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    state.scheduler.reload_bandwidth().await?;
    Ok(Json(ScheduleResponse {
        timezone: request.timezone,
        default_profile_id: request.default_profile_id,
        windows: stored,
    }))
}

#[utoipa::path(get, path = "/api/v1/bandwidth/status", tag = "bandwidth", responses((status = 200, body = BandwidthStatusResponse)))]
pub async fn bandwidth_status(
    State(state): State<AppState>,
) -> Result<Json<BandwidthStatusResponse>, ApiError> {
    let status = state.scheduler.bandwidth().status().await;
    let limits = status
        .active_profile
        .as_ref()
        .map(rd_limits::BandwidthProfile::budget_limits)
        .unwrap_or_default();
    let usage = |period: Option<&rd_limits::BudgetPeriod>, limit: Option<u64>| {
        period.map(|period| BudgetUsageResponse {
            period_key: period.key.clone(),
            used_bytes: rd_core::ByteCount::new(period.used_bytes).unwrap_or_default(),
            limit_bytes: limit.and_then(|value| rd_core::ByteCount::new(value).ok()),
        })
    };
    let binding = state
        .scheduler
        .bandwidth()
        .binding_limit(&rd_limits::TransferScope {
            kind: Some(rd_core::DownloadKind::Http),
            ..rd_limits::TransferScope::default()
        });
    Ok(Json(BandwidthStatusResponse {
        timezone: status.timezone,
        next_switch_at: status.next_switch_at,
        daily: usage(status.budget.as_ref().map(|b| &b.daily), limits.daily_bytes),
        monthly: usage(
            status.budget.as_ref().map(|b| &b.monthly),
            limits.monthly_bytes,
        ),
        budget_exhausted: status.exceeded.is_some(),
        binding_limit: binding.map(|limit| BindingLimitResponse {
            bytes_per_second: rd_core::ByteCount::new(limit.bytes_per_second).unwrap_or_default(),
            source: limit.source,
        }),
        active_profile: status.active_profile,
    }))
}

/// What each transport can actually enforce, so the UI can mark what a limit will not reach
/// instead of accepting it and silently ignoring it.
#[utoipa::path(get, path = "/api/v1/bandwidth/capabilities", tag = "bandwidth", responses((status = 200, body = [RunnerLimitSupport])))]
pub async fn bandwidth_capabilities() -> Json<Vec<RunnerLimitSupport>> {
    Json(rd_limits::limit_capabilities())
}

fn validated_profile(
    request: BandwidthProfileRequest,
) -> Result<rd_db::NewBandwidthProfile, ApiError> {
    let name = request.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request(
            "bandwidth.name_invalid",
            "A profile name must be between 1 and 100 characters",
        ));
    }
    if request.scopes.len() > MAX_SCOPES {
        return Err(
            ApiError::bad_request("bandwidth.too_many_scopes", "Too many scope limits")
                .with_param("max", MAX_SCOPES),
        );
    }
    if request
        .scopes
        .iter()
        .any(|scope| scope.bytes_per_second == 0)
    {
        return Err(ApiError::bad_request(
            "bandwidth.scope_limit_invalid",
            "A scope limit must be greater than zero",
        ));
    }
    if request.max_active_files == Some(0) {
        return Err(ApiError::bad_request(
            "bandwidth.parallelism_invalid",
            "The parallelism override must be at least one",
        ));
    }
    Ok(rd_db::NewBandwidthProfile {
        name,
        download_bytes_per_second: request.download_bytes_per_second,
        upload_bytes_per_second: request.upload_bytes_per_second,
        max_active_files: request.max_active_files,
        daily_budget_bytes: request.daily_budget_bytes,
        monthly_budget_bytes: request.monthly_budget_bytes,
        scopes: request.scopes,
    })
}

fn profile_not_found() -> ApiError {
    ApiError::bad_request(
        "bandwidth.profile_not_found",
        "The schedule references a profile that does not exist",
    )
}

fn not_found(error: anyhow::Error) -> ApiError {
    crate::error_codes::store_not_found(
        &error,
        "bandwidth.profile_not_found",
        "Bandwidth profile not found",
    )
}
