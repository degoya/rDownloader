//! A pending restart and the restart itself (RD-1240-32): what `GET` and
//! `POST /api/v1/system/restart`, the tray's restart, the MCP tools and the automatic restart
//! (`restart_auto`) share.
//!
//! **Pending** is what runs only from the next start: every plugin whose version this start runs
//! differs from the one the next start runs (installed, updated, chosen, rolled back, put under
//! test or taken off it, by hand or by the automatic plugin update) -- read from the plugin
//! manager's own `restart_required`, so the two never disagree -- and what the routes recorded
//! that the plugin manager does not show (`rd_api_core::restart_state`). What the start found
//! already is left out: a restart would not change it.
//!
//! **The restart** goes the way `rd_update::restart` decides for this installation: the
//! relauncher for a portable archive or the Windows installer started by hand or at login, the
//! supervisor's restart on exit code 75 under systemd or in a container. It is refused while an
//! update is being installed (`restart.update_running`), once one began
//! (`restart.already_restarting`), and while downloads run unless the caller agrees
//! (`restart.transfers_active`): the stop saves them, and they continue after the restart. It is
//! recorded in the audit log as a stop request with `restart` set, and announced as
//! `service_restarting` before the service stops.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::Utc;
use rd_api_core::notify_notice::{Notice, announce};
use rd_core::AuditAction;
use rd_update::restart::{DEFAULT_HEALTH_TIMEOUT_SECS, RestartHow, RestartPlan};

use crate::audit::{AuditContext, AuditEvent};
use crate::dto::{RestartReason, RestartRequest, RestartStartedResponse, RestartStatusResponse};
use crate::{ApiError, AppState};

/// Held from the first check of a request to its recorded start, as for an update install.
static STARTING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Where a pending restart stands, as every caller reads it.
pub async fn status(state: &AppState) -> RestartStatusResponse {
    let restart = &state.restart;
    let mut reasons: Vec<RestartReason> = derived(state)
        .await
        .into_iter()
        .filter(|reason| !restart.in_baseline(reason))
        .collect();
    // A plugin removed is derived while another version of it is left; recorded, it is told once.
    for recorded in restart.recorded() {
        let told = reasons
            .iter()
            .any(|reason| reason.code == recorded.code && reason.plugin_id == recorded.plugin_id);
        if !told {
            reasons.push(recorded);
        }
    }
    let (how, supervisor) = restart.how(state.updates.install_kind());
    let blocked_reason = blocked(state).map(str::to_owned);
    RestartStatusResponse {
        pending: !reasons.is_empty(),
        reasons,
        can_restart: blocked_reason.is_none(),
        how: how.as_str().to_owned(),
        supervisor: supervisor.map(|supervisor| supervisor.as_str().to_owned()),
        blocked_reason,
        restarting: restart.restarting(),
        automatic: state.updates.settings().await.restart_when_needed,
        started_at: restart.started_at().to_rfc3339(),
    }
}

/// Keeps what the derivation finds now as what a restart does not change. Called once at the
/// start, before anything could be installed.
pub async fn remember_baseline(state: &AppState) {
    state.restart.remember_baseline(derived(state).await);
}

/// The installed name of plugin `id`, for a reason a route records.
pub(crate) async fn plugin_name(state: &AppState, id: &str) -> Option<String> {
    state
        .plugins
        .list_installed()
        .await
        .ok()?
        .into_iter()
        .find(|manifest| manifest.id.to_string() == id)
        .map(|manifest| manifest.name)
}

/// Records that `code` about plugin `id` waits for the next start, named as installed.
pub(crate) async fn record_plugin(state: &AppState, code: &str, id: &str, version: Option<&str>) {
    let name = plugin_name(state, id).await;
    state
        .restart
        .record(RestartReason::plugin(code, id, name.as_deref(), version));
}

/// Why a restart cannot begin now, if it cannot.
fn blocked(state: &AppState) -> Option<&'static str> {
    if state.restart.restarting() {
        Some("restart.already_restarting")
    } else if state.updates.install_busy() {
        Some("restart.update_running")
    } else {
        None
    }
}

/// Every plugin whose next start runs something else than this one, from the plugin manager's
/// lifecycle. A plugin list that cannot be read reports nothing rather than a restart for nothing.
async fn derived(state: &AppState) -> Vec<RestartReason> {
    let versions = match crate::plugin_lifecycle::loadable_versions(state).await {
        Ok(versions) => versions,
        Err(error) => {
            tracing::warn!(error = %error.message(), "the plugin versions could not be read for the restart status");
            return Vec::new();
        }
    };
    let lifecycles = match crate::plugin_lifecycle::lifecycles(state, &versions).await {
        Ok(lifecycles) => lifecycles,
        Err(error) => {
            tracing::warn!(error = %error.message(), "the plugin lifecycles could not be read for the restart status");
            return Vec::new();
        }
    };
    let names: BTreeMap<String, String> = state
        .plugins
        .list_installed()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|manifest| (manifest.id.to_string(), manifest.name))
        .collect();
    lifecycles
        .into_iter()
        .filter(|lifecycle| lifecycle.restart_required)
        .map(|lifecycle| {
            let name = names.get(&lifecycle.plugin_id).map(String::as_str);
            let id = lifecycle.plugin_id.as_str();
            let next = lifecycle.active_version.as_deref();
            match (lifecycle.running_version.as_deref(), next) {
                (None, _) => RestartReason::plugin("plugin_installed", id, name, next),
                (Some(now), None) => RestartReason::plugin("plugin_removed", id, name, Some(now)),
                (Some(now), Some(next)) if now != next => RestartReason {
                    from_version: Some(now.to_owned()),
                    ..RestartReason::plugin("plugin_updated", id, name, Some(next))
                },
                _ => match lifecycle.staged_version.as_deref() {
                    Some(staged) => RestartReason::plugin("plugin_staged", id, name, Some(staged)),
                    None => RestartReason::plugin("plugin_unstaged", id, name, None),
                },
            }
        })
        .collect()
}

/// Begins a restart; see the module documentation. `automatic` records the service's own
/// decision (`restart_when_needed`) rather than a person's.
///
/// # Errors
///
/// `409` with `restart.update_running`, `restart.already_restarting`, `restart.transfers_active`
/// or `restart.relaunch_failed`.
pub async fn begin(
    state: &AppState,
    request: RestartRequest,
    audit: &AuditContext,
    automatic: bool,
) -> Result<RestartStartedResponse, ApiError> {
    let _starting = STARTING.lock().await;
    if let Some(code) = blocked(state) {
        let message = if code == "restart.update_running" {
            "An update is being installed; it restarts the service itself"
        } else {
            "rDownloader is restarting already"
        };
        return Err(ApiError::conflict(code, message));
    }
    let active = state.scheduler.transfer_rates().len();
    if active > 0 && !request.allow_active {
        return Err(ApiError::conflict(
            "restart.transfers_active",
            format!(
                "{active} downloads are running; they are saved by the stop and continue after \
                 the restart. Send allow_active to restart anyway"
            ),
        )
        .with_param("count", active));
    }
    let (how, supervisor) = state.restart.how(state.updates.install_kind());
    let reasons = status(state).await.reasons;
    if !state.restart.begin(how) {
        return Err(ApiError::conflict(
            "restart.already_restarting",
            "rDownloader is restarting already",
        ));
    }
    crate::audit::record(
        state,
        AuditEvent::success(AuditAction::ServiceStopRequested)
            .by(audit)
            .target("service", "rdownloader")
            .detail("restart", true)
            .detail("how", how.as_str())
            .detail("automatic", automatic)
            .detail("active_downloads", active)
            .detail("reasons", reasons.len()),
    )
    .await;
    // Queued before the stop: the delivery after the restart finds it waiting.
    announce(
        &state.database,
        Notice::service_restarting(&Utc::now().to_rfc3339(), &describe(&reasons), automatic),
    )
    .await;
    tracing::info!(how = how.as_str(), automatic, "rDownloader restarts");
    match how {
        RestartHow::Relaunch => {
            let launched = plan(state).and_then(|plan| state.restart.relaunch(&plan));
            if let Err(error) = launched {
                state.restart.abandon();
                tracing::error!(error = %format!("{error:#}"), "the relauncher could not be started; rDownloader keeps running");
                return Err(ApiError::conflict(
                    "restart.relaunch_failed",
                    format!("rDownloader could not start its relauncher: {error:#}"),
                ));
            }
        }
        // The graceful stop lets the request that asked finish; `serve` then ends with the
        // restart's exit code, and the supervisor starts it again.
        RestartHow::Supervisor | RestartHow::Manual => state.shutdown.cancel(),
    }
    Ok(RestartStartedResponse {
        how: how.as_str().to_owned(),
        supervisor: supervisor.map(|supervisor| supervisor.as_str().to_owned()),
    })
}

/// What the relauncher needs, every path absolute: this executable, its arguments and folder.
fn plan(state: &AppState) -> anyhow::Result<RestartPlan> {
    let executable = std::env::current_exe()?;
    let service_args = std::env::args_os()
        .skip(1)
        .map(|argument| {
            argument.into_string().map_err(|argument| {
                anyhow::anyhow!("the service argument {argument:?} is not text")
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(RestartPlan {
        version: state.updates.current_version().to_owned(),
        executable: std::path::absolute(&executable).unwrap_or(executable),
        data_dir: state.updates.data_dir(),
        service_pid: std::process::id(),
        service_args,
        service_cwd: std::env::current_dir()?,
        health_timeout_secs: DEFAULT_HEALTH_TIMEOUT_SECS,
        requested_at: Utc::now(),
    })
}

/// The reasons in words, for the notice: "Rapidgator 1.3.0, Example".
fn describe(reasons: &[RestartReason]) -> String {
    if reasons.is_empty() {
        return "what waits for the next start".to_owned();
    }
    reasons
        .iter()
        .map(|reason| {
            let subject = reason
                .name
                .as_deref()
                .or(reason.plugin_id.as_deref())
                .unwrap_or("a plugin");
            match reason.version.as_deref() {
                Some(version) => format!("{subject} {version}"),
                None => subject.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
