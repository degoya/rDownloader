//! Component schemas of the capture agent's game mode (RD-1240-19) and the server update and
//! restart its tray offers (RD-1240-25, RD-1240-32), apart from the shared ones in
//! `schemas.rs` so neither file outgrows the 500-line rule.

use utoipa::OpenApi;

use crate::{capture_game_mode, capture_server_update};

/// The game mode settings, the agent's hold and release and the tray's switch; the server update.
#[derive(OpenApi)]
#[openapi(components(schemas(
    rd_core::CaptureGameMode,
    rd_core::GameModeAction,
    capture_game_mode::CaptureGameModeHoldRequest,
    capture_game_mode::CaptureGameModeHoldResponse,
    capture_game_mode::CaptureGameModeReleaseRequest,
    capture_game_mode::CaptureGameModeReleaseResponse,
    capture_game_mode::CaptureGameModeSwitchRequest,
    capture_server_update::CaptureServerUpdateResponse,
    capture_server_update::CaptureServerUpdateOffer,
    capture_server_update::CaptureServerUpdateInstall,
    capture_server_update::CaptureServerRestart,
)))]
pub(crate) struct CaptureSchemas;
