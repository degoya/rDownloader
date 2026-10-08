//! What the desktop capture agent is set to from here: clipboard watching paused or not
//! (RD-1180-01), and the system-wide shortcuts of its tray commands (RD-1180-03).
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

use axum::{Json, extract::State, http::StatusCode};
use rd_core::{CaptureAgentSettings, CaptureShortcutReport, CaptureShortcuts};
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

async fn stored_report(
    database: &rd_db::Database,
) -> Result<Option<CaptureShortcutReport>, ApiError> {
    Ok(database
        .get_setting(SHORTCUT_REPORT_KEY)
        .await?
        .and_then(|value| serde_json::from_value(value).ok()))
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
    state
        .database
        .set_setting(
            CAPTURE_AGENT_SETTINGS_KEY.to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    announce(state);
    Ok(settings)
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

/// Pauses or resumes clipboard watching, or replaces the shortcuts; the agent follows within
/// seconds, without a restart.
#[utoipa::path(patch, path = "/api/v1/settings/capture-agent", tag = "capture", request_body = CaptureAgentSettingsPatch, responses((status = 200, body = CaptureAgentSettingsResponse), (status = 400, body = crate::error::ErrorBody)))]
pub async fn update_capture_agent_settings(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(patch): Json<CaptureAgentSettingsPatch>,
) -> Result<Json<CaptureAgentSettingsResponse>, ApiError> {
    let fields: Vec<&str> = [
        patch.clipboard_paused.map(|_| "clipboard_paused"),
        patch.shortcuts.as_ref().map(|_| "shortcuts"),
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
#[utoipa::path(get, path = "/api/v1/capture/agent-settings", tag = "capture", responses((status = 200, body = CaptureAgentSettings), (status = 401, body = crate::error::ErrorBody)))]
pub async fn read_capture_agent_settings(
    State(state): State<AppState>,
) -> Result<Json<CaptureAgentSettings>, ApiError> {
    Ok(Json(stored_capture_agent_settings(&state.database).await?))
}

/// The tray's "Pause clipboard watching" and `rdownloader-capture pause|resume`.
#[utoipa::path(post, path = "/api/v1/capture/clipboard", tag = "capture", request_body = CaptureClipboardRequest, responses((status = 200, body = CaptureAgentSettings), (status = 401, body = crate::error::ErrorBody)))]
pub async fn set_capture_clipboard(
    State(state): State<AppState>,
    Json(request): Json<CaptureClipboardRequest>,
) -> Result<Json<CaptureAgentSettings>, ApiError> {
    let settings = apply_capture_agent_patch(
        &state,
        CaptureAgentSettingsPatch {
            clipboard_paused: Some(request.paused),
            shortcuts: None,
        },
    )
    .await?;
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
