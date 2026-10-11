//! The service's own update, offered in the capture agent's tray (RD-1240-25).
//!
//! Reading is for every capture token: the offered version, how it is installed and where an
//! install stands, which is what the tray's "Install server update X" needs and nothing of the
//! administration around it -- no channel, no schedule, no download address. Installing is only
//! for an agent paired with `capture:server_update` (`rd_api_core::auth::
//! require_capture_server_update`), and it is the install the web interface starts
//! ([`crate::update_install_service::start`]): the same checks, the same refusals with their
//! codes, the same audit record, with the agent's token as its actor. An installation that does
//! not install itself -- a package manager's, a container's -- is refused there with
//! `update.install_unsupported`, and the tray shows its command instead of offering the install.
//!
//! A pending restart (RD-1240-32) is read here too, and the same right restarts the service from
//! the tray (`restart_service::begin`, the button's restart with the agent's token as its actor):
//! the tray's second line says a restart is pending, and "Restart server" appears.

use axum::{Extension, Json, extract::State, http::StatusCode};
use rd_api_core::auth::Granted;
use serde::Serialize;
use utoipa::ToSchema;

use crate::audit::AuditContext;
use crate::dto::{
    RestartRequest, RestartStartedResponse, UpdateInstallRequest, UpdateInstallStatus,
};
use crate::{ApiError, AppState};

/// What the tray reads about the service's update.
#[derive(Debug, Serialize, ToSchema)]
pub struct CaptureServerUpdateResponse {
    /// The newer version the service is offered; empty when there is none.
    pub available: Option<CaptureServerUpdateOffer>,
    /// Whether this agent may install it: it was paired with `capture:server_update`. Without it
    /// the tray opens the update page instead.
    pub may_install: bool,
    /// Where an install stands, or how the last one ended; empty when there is none to tell.
    pub install: Option<CaptureServerUpdateInstall>,
    /// Whether a restart is pending (RD-1240-32); `may_install` also says whether this agent
    /// may carry it out.
    pub restart: CaptureServerRestart,
}

/// A pending restart, as the tray shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct CaptureServerRestart {
    /// Something waits for the next start.
    pub pending: bool,
    /// How many things wait for it.
    pub reasons: usize,
    /// Whether a restart can begin now: no update is being installed, none runs already.
    pub can_restart: bool,
    /// `self`, `supervisor` or `manual`, as `GET /api/v1/system/restart` names it.
    pub how: String,
}

/// The offered version and how it gets installed.
#[derive(Debug, Serialize, ToSchema)]
pub struct CaptureServerUpdateOffer {
    pub version: String,
    /// `install` (the service installs it itself and restarts), `download` (it is replaced by
    /// hand) or `command` (a package manager or container runtime does it).
    pub action: String,
    /// The command to run, for `action` = `command`.
    pub command: Option<String>,
}

/// An install's state, as `GET /api/v1/system/update` names it, without its timestamps.
#[derive(Debug, Serialize, ToSchema)]
pub struct CaptureServerUpdateInstall {
    /// `downloading`, `preparing`, `restarting`, `installing`, `verifying`, `rolling_back`,
    /// `done`, `rolled_back` or `failed`.
    pub state: String,
    pub target_version: String,
    /// The stable code of why it failed or was rolled back.
    pub reason: Option<String>,
}

impl From<UpdateInstallStatus> for CaptureServerUpdateInstall {
    fn from(status: UpdateInstallStatus) -> Self {
        Self {
            state: status.state,
            target_version: status.target_version,
            reason: status.reason,
        }
    }
}

#[utoipa::path(get, path = "/api/v1/capture/server-update", tag = "capture", responses((status = 200, body = CaptureServerUpdateResponse)))]
pub async fn get_capture_server_update(
    State(state): State<AppState>,
    granted: Option<Extension<Granted>>,
) -> Json<CaptureServerUpdateResponse> {
    let status = state.updates.status().await;
    let restart = crate::restart_service::status(&state).await;
    Json(CaptureServerUpdateResponse {
        available: status.available.map(|offer| CaptureServerUpdateOffer {
            version: offer.version,
            action: offer.action,
            command: offer.command,
        }),
        // Read from the grant `require_capture` resolved for this request, like the summary's
        // queue control: the tray learns it may install from the lookup that will let it.
        may_install: granted
            .is_some_and(|Extension(granted)| granted.holds(rd_core::Scope::CaptureServerUpdate)),
        install: status.install.map(CaptureServerUpdateInstall::from),
        restart: CaptureServerRestart {
            pending: restart.pending,
            reasons: restart.reasons.len(),
            can_restart: restart.can_restart,
            how: restart.how,
        },
    })
}

/// Installs the offered update and restarts, as Settings > System > Updates does: answers at once
/// with the step it begins in, and `GET /api/v1/capture/server-update` follows it through the
/// restart. Running downloads refuse it with `update.transfers_active` unless `allow_active` is
/// sent; the stop saves the queue and they continue after the restart.
#[utoipa::path(
    post,
    path = "/api/v1/capture/server-update/install",
    tag = "capture",
    request_body = UpdateInstallRequest,
    responses(
        (status = 202, body = CaptureServerUpdateInstall),
        (status = 403, body = crate::error::ErrorBody, description = "The agent was not paired with capture:server_update"),
        (status = 409, body = crate::error::ErrorBody, description = "Nothing to install, not an installation that installs itself, downloads running, or the program folder is not fit")
    )
)]
pub async fn install_capture_server_update(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<UpdateInstallRequest>,
) -> Result<(StatusCode, Json<CaptureServerUpdateInstall>), ApiError> {
    let status = crate::update_install_service::start(&state, request, &audit).await?;
    tracing::info!(
        target = %status.target_version,
        "the capture agent started the server update from its tray"
    );
    Ok((StatusCode::ACCEPTED, Json(status.into())))
}

/// Restarts the service to apply what waits for the next start, as the button under Settings >
/// System > Updates does (RD-1240-32): answers at once with how it comes back, then stops.
/// Running downloads refuse it with `restart.transfers_active` unless `allow_active` is sent; the
/// stop saves them and they continue after the restart.
#[utoipa::path(
    post,
    path = "/api/v1/capture/server-update/restart",
    tag = "capture",
    request_body = RestartRequest,
    responses(
        (status = 202, body = RestartStartedResponse),
        (status = 403, body = crate::error::ErrorBody, description = "The agent was not paired with capture:server_update"),
        (status = 409, body = crate::error::ErrorBody, description = "An update is being installed, a restart runs already, downloads are running, or the relauncher could not be started")
    )
)]
pub async fn restart_capture_server(
    State(state): State<AppState>,
    audit: AuditContext,
    Json(request): Json<RestartRequest>,
) -> Result<(StatusCode, Json<RestartStartedResponse>), ApiError> {
    let started = crate::restart_service::begin(&state, request, &audit, false).await?;
    tracing::info!(how = %started.how, "the capture agent restarted the server from its tray");
    Ok((StatusCode::ACCEPTED, Json(started)))
}
