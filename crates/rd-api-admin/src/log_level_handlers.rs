//! The log level and the debug window over REST (RD-1260-12).
//!
//! The level is a field of the settings document, saved through the one settings path and
//! applied there; the route below is the Log view's shortcut to it. The debug window is kept
//! beside the settings (`rd_core::LOG_DEBUG_SETTING_KEY`), so the next start inside it opens it
//! again, and is closed by the filter's own timer at its end. With `RUST_LOG` set the filter is
//! the environment's: the routes that would change it answer `diagnostics.log_level_locked`.

use axum::{Json, extract::State};
use chrono::{Duration, Utc};
use rd_core::{LOG_DEBUG_MINUTES, LogDebugArea, LogDebugWindow, ServiceLogLevel};
use rd_diagnostics::log_filter::{self, ApplyError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState, error::ErrorBody, settings_store::read_settings};

/// The level, the open debug window and whether `RUST_LOG` decides instead.
#[derive(Serialize, ToSchema)]
pub struct LogLevelResponse {
    /// The level the settings hold.
    pub level: ServiceLogLevel,
    /// The open debug window, or `null`.
    pub debug: Option<LogDebugWindow>,
    /// `RUST_LOG` as the service found it at start. While it is set it is the filter, and the
    /// level and the window cannot be changed until the service starts without it.
    pub rust_log: Option<String>,
    /// The areas a debug window can open, in the order the interface offers them.
    pub areas: Vec<LogDebugArea>,
    /// The lengths a debug window may have, in minutes.
    pub minutes: Vec<u32>,
}

/// `PUT /api/v1/diagnostics/log-level`.
#[derive(Deserialize, ToSchema)]
pub struct LogLevelRequest {
    pub level: ServiceLogLevel,
}

/// `PUT /api/v1/diagnostics/log-level/debug`.
#[derive(Deserialize, ToSchema)]
pub struct LogDebugRequest {
    pub area: LogDebugArea,
    /// Only with the area `plugin`: an installed plugin's id, to put only that plugin's own
    /// lines on debug.
    #[serde(default)]
    pub plugin_id: Option<String>,
    /// 15, 30 or 60.
    pub minutes: u32,
}

/// The level and the window as they stand.
#[utoipa::path(get, path = "/api/v1/diagnostics/log-level", tag = "diagnostics", responses((status = 200, body = LogLevelResponse)))]
pub async fn get_log_level(
    State(state): State<AppState>,
) -> Result<Json<LogLevelResponse>, ApiError> {
    log_level_state(&state).await.map(Json)
}

/// Sets the level from the Log view: saved in the settings document and in force from the next
/// line, without a restart.
#[utoipa::path(put, path = "/api/v1/diagnostics/log-level", tag = "diagnostics", request_body = LogLevelRequest, responses((status = 200, body = LogLevelResponse), (status = 409, body = ErrorBody, description = "`RUST_LOG` decides the filter (`diagnostics.log_level_locked`)")))]
pub async fn set_log_level(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Json(request): Json<LogLevelRequest>,
) -> Result<Json<LogLevelResponse>, ApiError> {
    let holds_admin =
        granted.is_some_and(|axum::Extension(granted)| granted.holds(rd_core::Scope::Admin));
    change_log_level(&state, &audit, holds_admin, request.level)
        .await
        .map(Json)
}

/// Puts one area on debug for 15, 30 or 60 minutes; a window already open is replaced.
#[utoipa::path(put, path = "/api/v1/diagnostics/log-level/debug", tag = "diagnostics", request_body = LogDebugRequest, responses((status = 200, body = LogLevelResponse), (status = 400, body = ErrorBody, description = "`diagnostics.debug_minutes_invalid` or `diagnostics.debug_plugin_invalid`"), (status = 409, body = ErrorBody, description = "`RUST_LOG` decides the filter (`diagnostics.log_level_locked`)")))]
pub async fn start_log_debug(
    State(state): State<AppState>,
    Json(request): Json<LogDebugRequest>,
) -> Result<Json<LogLevelResponse>, ApiError> {
    open_debug_window(&state, request).await.map(Json)
}

/// Ends the debug window now; the level holds again from the next line.
#[utoipa::path(delete, path = "/api/v1/diagnostics/log-level/debug", tag = "diagnostics", responses((status = 200, body = LogLevelResponse)))]
pub async fn stop_log_debug(
    State(state): State<AppState>,
) -> Result<Json<LogLevelResponse>, ApiError> {
    close_debug_window(&state).await.map(Json)
}

/// [`get_log_level`]'s answer, for the MCP tools too.
pub async fn log_level_state(state: &AppState) -> Result<LogLevelResponse, ApiError> {
    let level = read_settings(state).await?.log_level;
    let debug = log_filter::reopened(log_filter::stored_window(&state.database).await, Utc::now());
    Ok(LogLevelResponse {
        level,
        debug,
        rust_log: log_filter::controller().state().rust_log,
        areas: LogDebugArea::ALL.to_vec(),
        minutes: LOG_DEBUG_MINUTES.to_vec(),
    })
}

/// [`set_log_level`]'s work: the settings path saves, audits and applies it.
pub async fn change_log_level(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    holds_admin: bool,
    level: ServiceLogLevel,
) -> Result<LogLevelResponse, ApiError> {
    refuse_when_locked()?;
    let mut settings = read_settings(state).await?;
    settings.log_level = level;
    crate::settings_handlers::save_settings(state, audit, holds_admin, settings).await?;
    log_level_state(state).await
}

/// [`start_log_debug`]'s work: checked, kept for a restart, applied.
pub async fn open_debug_window(
    state: &AppState,
    request: LogDebugRequest,
) -> Result<LogLevelResponse, ApiError> {
    refuse_when_locked()?;
    if !LOG_DEBUG_MINUTES.contains(&request.minutes) {
        return Err(ApiError::bad_request(
            "diagnostics.debug_minutes_invalid",
            "A debug window lasts 15, 30 or 60 minutes",
        )
        .with_param("minutes", request.minutes));
    }
    if let Some(plugin_id) = &request.plugin_id {
        check_plugin(state, request.area, plugin_id).await?;
    }
    let window = LogDebugWindow {
        area: request.area,
        plugin_id: request.plugin_id,
        until: Utc::now() + Duration::minutes(i64::from(request.minutes)),
    };
    store_window(state, Some(&window)).await?;
    let level = read_settings(state).await?.log_level;
    report(log_filter::controller().apply(level, Some(window.clone())));
    tracing::info!(
        area = window.area.as_str(),
        plugin_id = window.plugin_id.as_deref().unwrap_or_default(),
        until = %window.until,
        "debug logging switched on for a while"
    );
    log_level_state(state).await
}

/// [`stop_log_debug`]'s work. Nothing open is no error: the answer says what stands.
pub async fn close_debug_window(state: &AppState) -> Result<LogLevelResponse, ApiError> {
    store_window(state, None).await?;
    let level = read_settings(state).await?.log_level;
    report(log_filter::controller().apply(level, None));
    log_level_state(state).await
}

/// Applies a level just saved, with the window still open. Called by the settings path, which
/// every change of the level goes through.
pub(crate) fn apply_level(level: ServiceLogLevel) {
    let controller = log_filter::controller();
    report(controller.apply(level, controller.state().debug));
}

/// The filter's refusal under `RUST_LOG` is expected and said by the routes; anything else is
/// a filter that did not change, and that goes to the log.
fn report<T>(result: Result<T, ApplyError>) {
    match result {
        Ok(_) | Err(ApplyError::LockedByEnvironment) => {}
        Err(error) => tracing::warn!(%error, "the log filter was not changed"),
    }
}

fn refuse_when_locked() -> Result<(), ApiError> {
    match log_filter::controller().state().rust_log {
        Some(rust_log) => Err(ApiError::conflict(
            "diagnostics.log_level_locked",
            "RUST_LOG decides the log filter; the level applies once the service starts without it",
        )
        .with_param("rust_log", rust_log)),
        None => Ok(()),
    }
}

/// A plugin id is taken with the plugin area only, and only for an installed plugin: it ends
/// up in a filter directive, so it is also checked for what could end one.
async fn check_plugin(
    state: &AppState,
    area: LogDebugArea,
    plugin_id: &str,
) -> Result<(), ApiError> {
    let refused = || {
        ApiError::bad_request(
            "diagnostics.debug_plugin_invalid",
            "Only the plugin area takes a plugin, and only an installed one",
        )
        .with_param("plugin_id", plugin_id)
    };
    if area != LogDebugArea::Plugin || !rd_core::is_directive_safe_plugin_id(plugin_id) {
        return Err(refused());
    }
    let installed = state.plugins.list_installed().await?;
    if !installed
        .iter()
        .any(|manifest| manifest.id.to_string() == plugin_id)
    {
        return Err(refused());
    }
    Ok(())
}

async fn store_window(state: &AppState, window: Option<&LogDebugWindow>) -> Result<(), ApiError> {
    let value = serde_json::to_value(window).map_err(anyhow::Error::new)?;
    state
        .database
        .set_setting(rd_core::LOG_DEBUG_SETTING_KEY.to_owned(), value)
        .await?;
    Ok(())
}
