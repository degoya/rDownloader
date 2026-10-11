//! What the desktop capture agent is set to from here: clipboard watching paused or not
//! (RD-1180-01), the system-wide shortcuts of its tray commands (RD-1180-03) and its game mode
//! (RD-1240-19; the hold itself and the tray's switch, RD-1240-23, are `capture_game_mode`).
//!
//! A settings row of its own rather than fields of the settings document. The tray switches the
//! clipboard pause while the settings page may be open with the whole document in memory, and
//! that page saves the whole document: a field in there was switched back by the next unrelated
//! save. Every write here is a patch of the one row instead, made under one lock.
//!
//! Two doors to it. The settings page and MCP read and patch it with `api:config`; the agent
//! reads it, switches the pause and reports its shortcut registrations with its capture token,
//! which reaches nothing else of the configuration. The agent follows the row on its five-second
//! poll, so a change made here reaches it without a restart.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use rd_core::{CaptureAgentSettings, CaptureGameMode, CaptureShortcutReport, CaptureShortcuts};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    audit::{AuditContext, AuditEvent},
};

/// The settings row the agent's settings live in.
pub const CAPTURE_AGENT_SETTINGS_KEY: &str = "capture.agent";
/// The row holding what an agent last said about its shortcut registrations.
const SHORTCUT_REPORT_KEY: &str = "capture.agent.shortcut_report";

/// Serializes the read-modify-write of the row: the tray's switch and the settings page may
/// patch it in the same moment, and each must see the other's change.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The agent's settings as the settings page shows them.
#[derive(Serialize, ToSchema)]
pub struct CaptureAgentSettingsResponse {
    pub clipboard_paused: bool,
    pub shortcuts: CaptureShortcuts,
    /// The built-in shortcuts, for "Reset".
    pub default_shortcuts: CaptureShortcuts,
    /// Pausing the queue or switching a profile while a game runs (RD-1240-19).
    pub game_mode: CaptureGameMode,
    /// What an agent last said about registering the shortcuts; `None` before any did.
    pub report: Option<CaptureShortcutReport>,
}

/// A change to the agent's settings; a field left out stays as it is.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct CaptureAgentSettingsPatch {
    #[serde(default)]
    pub clipboard_paused: Option<bool>,
    /// Replaces every shortcut; a command left out of the object takes its default, `null` is
    /// "no shortcut".
    #[serde(default)]
    pub shortcuts: Option<CaptureShortcuts>,
    /// Replaces the game mode as a whole (RD-1240-19).
    #[serde(default)]
    pub game_mode: Option<CaptureGameMode>,
}

/// The agent's own switch.
#[derive(Deserialize, ToSchema)]
pub struct CaptureClipboardRequest {
    /// Whether to leave the clipboard alone from now on.
    pub paused: bool,
}

/// The stored settings, or the defaults when nothing was ever stored.
///
/// A row that does not read is the defaults with a warning rather than a refusal: the agent
/// polls this every five seconds, and a broken row must not leave it without an answer.
pub async fn stored_capture_agent_settings(
    database: &rd_db::Database,
) -> Result<CaptureAgentSettings, ApiError> {
    let Some(value) = database.get_setting(CAPTURE_AGENT_SETTINGS_KEY).await? else {
        return Ok(CaptureAgentSettings::default());
    };
    Ok(serde_json::from_value(value).unwrap_or_else(|error| {
        tracing::warn!(%error, "the capture agent's stored settings do not read; using the defaults");
        CaptureAgentSettings::default()
    }))
}

/// The agent's last shortcut report; one that does not read is no report, with a warning
/// like the settings above (API-03).
async fn stored_report(
    database: &rd_db::Database,
) -> Result<Option<CaptureShortcutReport>, ApiError> {
    let Some(value) = database.get_setting(SHORTCUT_REPORT_KEY).await? else {
        return Ok(None);
    };
    Ok(serde_json::from_value(value)
        .inspect_err(|error| {
            tracing::warn!(%error, "the capture agent's stored shortcut report does not read; showing none");
        })
        .ok())
}

/// Applies a patch under the lock, stores the result and tells open pages.
///
/// Shortcuts are validated as a whole and stored in their canonical spelling; a refusal names
/// the command and, for a duplicate, the command that has the combination already.
pub async fn apply_capture_agent_patch(
    state: &AppState,
    patch: CaptureAgentSettingsPatch,
) -> Result<CaptureAgentSettings, ApiError> {
    let _guard = WRITE.lock().await;
    let mut settings = stored_capture_agent_settings(&state.database).await?;
    if let Some(paused) = patch.clipboard_paused {
        settings.clipboard_paused = paused;
    }
    if let Some(shortcuts) = patch.shortcuts {
        settings.shortcuts = shortcuts.validated().map_err(|refusal| {
            let mut error = ApiError::bad_request(
                refusal.problem.code(),
                format!(
                    "The shortcut of {} cannot be used",
                    refusal.command.as_str()
                ),
            )
            .with_param("command", refusal.command.as_str());
            if let Some(other) = refusal.other {
                error = error.with_param("other", other.as_str());
            }
            error
        })?;
    }
    if let Some(game_mode) = patch.game_mode {
        settings.game_mode = validated_game_mode(state, &game_mode).await?;
    }
    store(state, &settings).await?;
    Ok(settings)
}

/// Switches game mode on or off under the same lock, keeping its triggers and its action: the
/// tray's "Pause while gaming" (RD-1240-23).
pub async fn set_capture_game_mode_enabled(
    state: &AppState,
    enabled: bool,
) -> Result<CaptureAgentSettings, ApiError> {
    let _guard = WRITE.lock().await;
    let mut settings = stored_capture_agent_settings(&state.database).await?;
    settings.game_mode.enabled = enabled;
    store(state, &settings).await?;
    Ok(settings)
}

/// Writes the row and tells open pages; the caller holds [`WRITE`].
async fn store(state: &AppState, settings: &CaptureAgentSettings) -> Result<(), ApiError> {
    state
        .database
        .set_setting(
            CAPTURE_AGENT_SETTINGS_KEY.to_owned(),
            serde_json::to_value(settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    announce(state);
    Ok(())
}

/// The game mode as stored: names trimmed and once each, and a profile that exists.
async fn validated_game_mode(
    state: &AppState,
    game_mode: &CaptureGameMode,
) -> Result<CaptureGameMode, ApiError> {
    let stored = game_mode.validated().map_err(|problem| {
        ApiError::bad_request(problem.code(), "The game mode settings cannot be used")
            .with_param("max_processes", rd_core::MAX_GAME_MODE_PROCESSES)
    })?;
    if let Some(id) = stored.profile_id {
        let profiles = state.database.list_bandwidth_profiles().await?;
        if !profiles.iter().any(|profile| profile.id == id) {
            return Err(ApiError::bad_request(
                "bandwidth.profile_not_found",
                "Bandwidth profile not found",
            ));
        }
    }
    Ok(stored)
}

/// Open settings pages reload on this; the capture event stream does not carry it.
fn announce(state: &AppState) {
    state.database.broadcast(rd_core::EventEnvelope::new(
        rd_core::EventKind::CaptureChanged,
        serde_json::json!({ "resource": "capture_agent_settings" }),
    ));
}

async fn response(
    state: &AppState,
    settings: CaptureAgentSettings,
) -> Result<CaptureAgentSettingsResponse, ApiError> {
    Ok(CaptureAgentSettingsResponse {
        clipboard_paused: settings.clipboard_paused,
        shortcuts: settings.shortcuts,
        default_shortcuts: CaptureShortcuts::default(),
        game_mode: settings.game_mode,
        report: stored_report(&state.database).await?,
    })
}

/// Whether the agent watches the clipboard, its shortcuts and what it reported about them.
#[utoipa::path(get, path = "/api/v1/settings/capture-agent", tag = "capture", responses((status = 200, body = CaptureAgentSettingsResponse)))]
pub async fn get_capture_agent_settings(
    State(state): State<AppState>,
) -> Result<Json<CaptureAgentSettingsResponse>, ApiError> {
    let settings = stored_capture_agent_settings(&state.database).await?;
    Ok(Json(response(&state, settings).await?))
}

/// Pauses or resumes clipboard watching, replaces the shortcuts or the game mode; the agent
/// follows within seconds, without a restart.
#[utoipa::path(patch, path = "/api/v1/settings/capture-agent", tag = "capture", request_body = CaptureAgentSettingsPatch, responses((status = 200, body = CaptureAgentSettingsResponse), (status = 400, body = crate::error::ErrorBody)))]
pub async fn update_capture_agent_settings(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(patch): Json<CaptureAgentSettingsPatch>,
) -> Result<Json<CaptureAgentSettingsResponse>, ApiError> {
    let fields: Vec<&str> = [
        patch.clipboard_paused.map(|_| "clipboard_paused"),
        patch.shortcuts.as_ref().map(|_| "shortcuts"),
        patch.game_mode.as_ref().map(|_| "game_mode"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let settings = apply_capture_agent_patch(&state, patch).await?;
    crate::audit::record(
        &state,
        AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(&audit)
            .target("settings", CAPTURE_AGENT_SETTINGS_KEY)
            .detail("fields", fields.join(" ")),
    )
    .await;
    Ok(Json(response(&state, settings).await?))
}

/// The agent's poll: what it is set to.
///
/// It carries the agent's own update report on the way in and the service's update channel on
/// the way out (RD-1210-03, `rd_update::agent::report`): an agent installed without the service
/// reads the channel the service reads, and the update status shows what the agent offers itself.
#[utoipa::path(get, path = "/api/v1/capture/agent-settings", tag = "capture", responses((status = 200, body = CaptureAgentSettings, headers(("x-rdownloader-update-channel" = String, description = "The service's update channel, stable or beta"))), (status = 401, body = crate::error::ErrorBody)))]
pub async fn read_capture_agent_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    state.capture_agents.note_report(&headers);
    let channel = state.updates.settings().await.channel();
    Ok((
        [(rd_update::agent::report::CHANNEL_HEADER, channel.as_str())],
        Json(stored_capture_agent_settings(&state.database).await?),
    ))
}

/// The tray's "Pause clipboard watching" and `rdownloader-capture pause|resume`.
///
/// Audited like the settings page's switch (audit 2026-10-08, API-02): the same row changes,
/// and the record names the capture token that changed it.
#[utoipa::path(post, path = "/api/v1/capture/clipboard", tag = "capture", request_body = CaptureClipboardRequest, responses((status = 200, body = CaptureAgentSettings), (status = 401, body = crate::error::ErrorBody)))]
pub async fn set_capture_clipboard(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<CaptureClipboardRequest>,
) -> Result<Json<CaptureAgentSettings>, ApiError> {
    let settings = apply_capture_agent_patch(
        &state,
        CaptureAgentSettingsPatch {
            clipboard_paused: Some(request.paused),
            ..CaptureAgentSettingsPatch::default()
        },
    )
    .await?;
    crate::audit::record(
        &state,
        AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(&audit)
            .target("settings", CAPTURE_AGENT_SETTINGS_KEY)
            .detail("fields", "clipboard_paused"),
    )
    .await;
    Ok(Json(settings))
}

/// What the agent found when it registered the shortcuts: which the system refused, or that it
/// could register none. The settings page shows it beside the fields.
#[utoipa::path(post, path = "/api/v1/capture/shortcut-report", tag = "capture", request_body = CaptureShortcutReport, responses((status = 204), (status = 401, body = crate::error::ErrorBody)))]
pub async fn report_capture_shortcuts(
    State(state): State<AppState>,
    Json(mut report): Json<CaptureShortcutReport>,
) -> Result<StatusCode, ApiError> {
    // As long as the command list at most, whatever arrived.
    report.refused.sort_unstable();
    report.refused.dedup();
    report.reported_at = Some(chrono::Utc::now());
    state
        .database
        .set_setting(
            SHORTCUT_REPORT_KEY.to_owned(),
            serde_json::to_value(&report).map_err(anyhow::Error::new)?,
        )
        .await?;
    announce(&state);
    Ok(StatusCode::NO_CONTENT)
}
