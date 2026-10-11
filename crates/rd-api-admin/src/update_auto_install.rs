//! The automatic install of an offered update (RD-1240-27): the loop that installs it by itself
//! when `update_auto_install` is on, and the notices around every install.
//!
//! When is `rd_update::auto_install`'s decision; this observes what it asks about once a
//! [`TICK`] — the setting, whether this installation installs itself, the offer the status
//! shows (so a beta is never installed on the stable channel), whether a transfer, a
//! post-processing step or a recording runs, the local time in the installation's time zone
//! (`bandwidth_timezone`) — and installs through `update_install_service`, the same steps as the
//! click: the verified download, the backup before the update, the updater with its roll-back.
//! A seeding torrent does not hold an install back: it is saved by the stop like every other
//! entry and seeds on after the restart.
//!
//! The start of an automatic install is announced before the service stops
//! (`Notice::update_installing`) and recorded in the audit log as the service's own; whatever an
//! install comes to — the new version running, or one that did not go ahead or was taken back —
//! is announced once per version, for an install a person started as well.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rd_api_core::notify_notice::{Notice, announce};
use rd_core::AuditAction;
use rd_update::auto_install::{AutoInstall, Moment, Wait};

use crate::audit::{Actor, AuditContext, AuditEvent};
use crate::dto::UpdateInstallRequest;
use crate::{ApiError, AppState};

/// How often the loop looks.
pub const TICK: Duration = Duration::from_secs(60);

/// What one look came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tick {
    /// The install of this version started.
    Started(String),
    /// Nothing to do yet, and why.
    Waiting(Wait),
    /// The install was refused at its start, with this code.
    Refused(String),
}

/// The loop's memory between two looks: the quiet clock, and the outcome last announced.
#[derive(Debug, Default)]
pub struct AutoInstaller {
    decision: AutoInstall,
    announced: Option<String>,
}

/// Starts the loop. Returns at once; the first look is a [`TICK`] after the start.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let mut installer = AutoInstaller::default();
        loop {
            tokio::time::sleep(TICK).await;
            installer.tick(&state, Utc::now()).await;
        }
    });
}

impl AutoInstaller {
    /// One look at `now`: announces how the last install ended, then installs when the time has
    /// come.
    pub async fn tick(&mut self, state: &AppState, now: DateTime<Utc>) -> Tick {
        self.announce_outcome(state).await;
        let updates = &state.updates;
        let settings = updates.settings().await;
        let offered = updates
            .status()
            .await
            .available
            .filter(|offer| offer.action == "install")
            .map(|offer| offer.version);
        let outcome = updates.install_status();
        let failed_before = outcome.as_ref().is_some_and(|status| {
            matches!(status.state.as_str(), "failed" | "rolled_back")
                && offered.as_deref() == Some(status.target_version.as_str())
        });
        let moment = Moment {
            enabled: settings.update_auto_install,
            installs_itself: updates.installs_itself(),
            offered: offered.as_deref(),
            installing: updates.install_busy(),
            failed_before,
            busy: busy(state).await,
            local_minute: local_minute(state, now).await,
            window: settings.auto_install_window(),
        };
        let version = match self.decision.decide(&moment, now) {
            Ok(version) => version.to_owned(),
            Err(wait) => {
                if !matches!(wait, Wait::Off | Wait::NothingOffered) {
                    tracing::debug!(wait = wait.as_str(), "the automatic update waits");
                }
                return Tick::Waiting(wait);
            }
        };
        install(state, &version).await
    }

    /// Announces the end of the install the status shows, once per version: the new version
    /// running, or one that did not go ahead or was taken back.
    async fn announce_outcome(&mut self, state: &AppState) {
        let Some(status) = state.updates.install_status() else {
            return;
        };
        let current = state.updates.current_version();
        let notice = match status.state.as_str() {
            "done" => Notice::update_installed(&status.target_version, &status.from_version),
            "failed" | "rolled_back" => Notice::update_failed(
                &status.target_version,
                current,
                status.reason.as_deref().unwrap_or("update.failed"),
            ),
            _ => return,
        };
        if self.announced.as_deref() == Some(notice.key.as_str()) {
            return;
        }
        self.announced = Some(notice.key.clone());
        announce(&state.database, notice).await;
    }
}

/// Installs `version` as the service's own decision, announced before the service stops.
async fn install(state: &AppState, version: &str) -> Tick {
    let audit = AuditContext {
        actor: Actor::system(),
        trace: None,
    };
    let current = state.updates.current_version().to_owned();
    tracing::info!(target = %version, "nothing has run for a while; installing the update automatically");
    // Queued before the start: the updater stops this service soon after, and the delivery
    // after the restart finds it waiting.
    announce(
        &state.database,
        Notice::update_installing(version, &current),
    )
    .await;
    match crate::update_install_service::begin(state, UpdateInstallRequest::default(), &audit, true)
        .await
    {
        Ok(_) => Tick::Started(version.to_owned()),
        Err(error) => refused(state, version, &current, &error).await,
    }
}

/// Records and announces an automatic install refused at its start: a download began in the
/// moment between the look and the start, the folder is not writable, the disk is full.
async fn refused(state: &AppState, version: &str, current: &str, error: &ApiError) -> Tick {
    let code = error.code().to_owned();
    tracing::warn!(target = %version, code, error = %error.message(), "the automatic update did not start");
    crate::audit::record(
        state,
        AuditEvent::failure(AuditAction::UpdateInstallStarted)
            .actor(Actor::system())
            .target("update", version)
            .detail("from_version", current)
            .detail("automatic", true)
            .detail("code", &code),
    )
    .await;
    // A download that started in between is no failure: the next quiet moment tries again.
    if code != "update.transfers_active" {
        announce(
            &state.database,
            Notice::update_failed(version, current, &code),
        )
        .await;
    }
    Tick::Refused(code)
}

/// Whether anything runs that an install would interrupt: a transfer, a post-processing step
/// (verifying, repairing, unpacking, or a package waiting for its unpack), or a recording. A
/// queue that cannot be read counts as running. The automatic restart (RD-1240-32,
/// `restart_auto`) asks the same.
pub(crate) async fn busy(state: &AppState) -> bool {
    if !state.scheduler.transfer_rates().is_empty() || !state.extraction.pending().await.is_empty()
    {
        return true;
    }
    match state.database.list_downloads().await {
        Ok(downloads) => downloads.iter().any(|file| file.state.is_working()),
        Err(error) => {
            tracing::warn!(%error, "the queue could not be read; it counts as running");
            true
        }
    }
}

/// Minutes after midnight in the installation's time zone (`bandwidth_timezone`).
pub(crate) async fn local_minute(state: &AppState, now: DateTime<Utc>) -> u16 {
    let timezone = match state
        .database
        .get_setting(rd_db::SERVICE_SETTINGS_KEY)
        .await
    {
        Ok(Some(blob)) => rd_db::service_setting_field_of::<String>(&blob, "bandwidth_timezone")
            .and_then(|value| rd_limits::parse_timezone(&value).ok()),
        _ => None,
    }
    .unwrap_or_else(rd_limits::default_timezone);
    rd_limits::local_position(timezone, now).1
}
