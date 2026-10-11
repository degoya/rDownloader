//! The agent's game mode (RD-1240-19): the hold it asks for while a full-screen program or a
//! named process runs, and the release once it ended.
//!
//! What a hold does is read here, from the agent's settings row, not from the request: an agent
//! paired with queue control can pause the queue already, and this way it can switch no profile
//! but the one the settings page chose. Each hold is timed ([`rd_core::GAME_MODE_HOLD_MINUTES`])
//! and renewed by the agent while the trigger lasts, so a hold whose agent vanished ends by
//! itself.
//!
//! Only the agent's own hold is renewed or lifted: one that ends exactly when the last answer
//! said. A pause or a switch somebody made, changed or ended in the meantime is theirs, and the
//! agent leaves it alone until its trigger is over (`rd_core::GameModeInForce`). The check and
//! the change are two steps; what can come between them is one request of somebody else.
//!
//! The tray's "Pause while gaming" (RD-1240-23) switches game mode on and off with the same right
//! and keeps its triggers. Switching off lifts nothing here: only the agent knows the end of its
//! hold, and it lifts the hold itself as soon as it has the switch.

use axum::{Json, extract::State};
use chrono::{DateTime, Duration, SubsecRound, Utc};
use rd_core::{CaptureAgentSettings, GameModeAction, GameModeInForce, GameModeProblem};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    audit::{AuditContext, AuditEvent},
    capture_agent_handlers::{
        CAPTURE_AGENT_SETTINGS_KEY, set_capture_game_mode_enabled, stored_capture_agent_settings,
    },
};

#[derive(Deserialize, ToSchema)]
pub struct CaptureGameModeHoldRequest {
    /// The end the previous answer gave, when this renews the agent's own hold; absent for a
    /// new one.
    #[serde(default)]
    pub renews: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CaptureGameModeHoldResponse {
    /// Whether the hold is set. `false` when somebody else's pause or switch holds, when the
    /// agent's own was ended or changed meanwhile, or when game mode is off.
    pub held: bool,
    /// When the hold ends unless it is renewed; the agent sends it back to renew or lift it.
    pub until: Option<DateTime<Utc>>,
    /// What the hold is: the queue paused or the profile switched on.
    pub action: GameModeAction,
}

#[derive(Deserialize, ToSchema)]
pub struct CaptureGameModeReleaseRequest {
    /// The end of the agent's own hold, as the last hold answer gave it.
    pub until: DateTime<Utc>,
}

/// The tray's switch (RD-1240-23).
#[derive(Deserialize, ToSchema)]
pub struct CaptureGameModeSwitchRequest {
    /// Whether game mode is switched on; the programs, full screen and the action stay as they
    /// are.
    pub enabled: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct CaptureGameModeReleaseResponse {
    /// Whether a hold of the agent's was lifted; `false` when nothing of its own held any more.
    pub released: bool,
}

/// What pauses the whole queue now.
async fn queue_in_force(state: &AppState) -> GameModeInForce {
    match state.scheduler.queue_pause().await {
        None => GameModeInForce::Nothing,
        Some(pause) => pause
            .until
            .map_or(GameModeInForce::Open, GameModeInForce::Until),
    }
}

/// What stands in front of the bandwidth schedule now.
async fn switch_in_force(state: &AppState) -> GameModeInForce {
    match state.scheduler.bandwidth().status().await.manual {
        None => GameModeInForce::Nothing,
        Some(manual) => manual
            .until
            .map_or(GameModeInForce::Open, GameModeInForce::Until),
    }
}

/// Sets or renews the agent's hold: the timed pause of the whole queue, or the chosen bandwidth
/// profile switched on by hand, for the next few minutes.
#[utoipa::path(post, path = "/api/v1/capture/game-mode/hold", tag = "capture", request_body = CaptureGameModeHoldRequest, responses((status = 200, body = CaptureGameModeHoldResponse), (status = 400, body = crate::error::ErrorBody), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn hold_capture_game_mode(
    State(state): State<AppState>,
    Json(request): Json<CaptureGameModeHoldRequest>,
) -> Result<Json<CaptureGameModeHoldResponse>, ApiError> {
    let mode = stored_capture_agent_settings(&state.database)
        .await?
        .game_mode;
    let refused = CaptureGameModeHoldResponse {
        held: false,
        until: None,
        action: mode.action,
    };
    if !mode.active() {
        return Ok(Json(refused));
    }
    // Whole seconds, so the end the agent sends back compares equal whatever carried it.
    let end = (Utc::now() + Duration::minutes(i64::from(rd_core::GAME_MODE_HOLD_MINUTES)))
        .trunc_subsecs(0);
    let until = match mode.action {
        GameModeAction::Pause => {
            if !queue_in_force(&state).await.may_hold(request.renews) {
                return Ok(Json(refused));
            }
            state.scheduler.pause_queue_until(end).await?.until
        }
        GameModeAction::Profile => {
            let problem = GameModeProblem::ProfileMissing;
            let profile_id = mode.profile_id.ok_or_else(|| {
                ApiError::bad_request(problem.code(), "Game mode names no bandwidth profile")
            })?;
            let profiles = state.database.list_bandwidth_profiles().await?;
            if !profiles.iter().any(|profile| profile.id == profile_id) {
                return Err(ApiError::bad_request(
                    "bandwidth.profile_not_found",
                    "Bandwidth profile not found",
                ));
            }
            if !switch_in_force(&state).await.may_hold(request.renews) {
                return Ok(Json(refused));
            }
            state
                .scheduler
                .switch_bandwidth_profile(Some(profile_id), rd_limits::ManualEnd::At, Some(end))
                .await?
                .until
        }
    };
    tracing::info!(action = ?mode.action, ?until, renewed = request.renews.is_some(), "the capture agent's game mode holds");
    Ok(Json(CaptureGameModeHoldResponse {
        held: true,
        until,
        action: mode.action,
    }))
}

/// Lifts the agent's own hold: the queue pause or the profile switch that ends at `until`. Both
/// are looked at, so a hold made before the settings page changed the action is lifted too.
#[utoipa::path(post, path = "/api/v1/capture/game-mode/release", tag = "capture", request_body = CaptureGameModeReleaseRequest, responses((status = 200, body = CaptureGameModeReleaseResponse), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn release_capture_game_mode(
    State(state): State<AppState>,
    Json(request): Json<CaptureGameModeReleaseRequest>,
) -> Result<Json<CaptureGameModeReleaseResponse>, ApiError> {
    let mut released = false;
    if queue_in_force(&state).await.is_own(request.until) {
        let resumed = state.scheduler.resume_queue().await?;
        tracing::info!(resumed, "the capture agent's game mode pause was lifted");
        released = true;
    }
    if switch_in_force(&state).await.is_own(request.until) {
        state.scheduler.return_to_bandwidth_schedule().await?;
        tracing::info!("the capture agent's game mode profile was lifted");
        released = true;
    }
    Ok(Json(CaptureGameModeReleaseResponse { released }))
}

/// The tray's "Pause while gaming" (RD-1240-23): switches game mode on or off and keeps what it
/// watches for. Audited like the settings page's change of the same row.
#[utoipa::path(post, path = "/api/v1/capture/game-mode", tag = "capture", request_body = CaptureGameModeSwitchRequest, responses((status = 200, body = CaptureAgentSettings), (status = 401, body = crate::error::ErrorBody), (status = 403, body = crate::error::ErrorBody)))]
pub async fn switch_capture_game_mode(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<CaptureGameModeSwitchRequest>,
) -> Result<Json<CaptureAgentSettings>, ApiError> {
    let settings = set_capture_game_mode_enabled(&state, request.enabled).await?;
    tracing::info!(
        enabled = request.enabled,
        "the capture agent switched its game mode"
    );
    crate::audit::record(
        &state,
        AuditEvent::success(rd_core::AuditAction::SettingsChanged)
            .by(&audit)
            .target("settings", CAPTURE_AGENT_SETTINGS_KEY)
            .detail("fields", "game_mode.enabled"),
    )
    .await;
    Ok(Json(settings))
}
