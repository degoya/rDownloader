//! The automatic restart (RD-1240-32): the loop that carries out a pending restart by itself
//! when `restart_when_needed` is on.
//!
//! When is `rd_update::auto_restart`'s decision, over the automatic install's quiet clock; this
//! observes what it asks about once a [`TICK`] -- the setting, whether a restart is pending and
//! can begin, whether a transfer, a post-processing step or a recording runs, the local time in
//! the installation's time zone, the automatic install's window -- with the same observations
//! the automatic install makes (`update_auto_install`), and restarts through
//! `restart_service::begin`, the same steps as the button, recorded as the service's own. With
//! automatic plugin updates on, this is what brings an updated plugin into service.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rd_update::auto_install::Wait;
use rd_update::auto_restart::{AutoRestart, RestartMoment};

use crate::AppState;
use crate::audit::{Actor, AuditContext};
use crate::dto::RestartRequest;

/// How often the loop looks.
pub const TICK: Duration = Duration::from_secs(60);

/// What one look came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Tick {
    /// The restart began, in this way (`self`, `supervisor` or `manual`).
    Started(String),
    /// Nothing to do yet, and why.
    Waiting(Wait),
    /// The restart was refused at its start, with this code.
    Refused(String),
}

/// The loop's memory between two looks: the quiet clock.
#[derive(Debug, Default)]
pub struct AutoRestarter {
    decision: AutoRestart,
}

/// Remembers what the start found pending already, then starts the loop. Returns at once; the
/// first look is a [`TICK`] after the start.
pub fn start(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        crate::restart_service::remember_baseline(&state).await;
        let mut restarter = AutoRestarter::default();
        loop {
            tokio::time::sleep(TICK).await;
            restarter.tick(&state, Utc::now()).await;
        }
    });
}

impl AutoRestarter {
    /// One look at `now`: restarts when the time has come.
    pub async fn tick(&mut self, state: &AppState, now: DateTime<Utc>) -> Tick {
        let settings = state.updates.settings().await;
        let status = crate::restart_service::status(state).await;
        let moment = RestartMoment {
            enabled: settings.restart_when_needed,
            pending: status.pending,
            can_restart: status.can_restart,
            busy: crate::update_auto_install::busy(state).await,
            local_minute: crate::update_auto_install::local_minute(state, now).await,
            window: settings.auto_install_window(),
        };
        if let Err(wait) = self.decision.decide(&moment, now) {
            if !matches!(wait, Wait::Off | Wait::NothingPending) {
                tracing::debug!(wait = wait.as_str(), "the automatic restart waits");
            }
            return Tick::Waiting(wait);
        }
        let audit = AuditContext {
            actor: Actor::system(),
            trace: None,
        };
        tracing::info!(
            reasons = status.reasons.len(),
            "nothing has run for a while; restarting to apply what waits for the next start"
        );
        match crate::restart_service::begin(state, RestartRequest::default(), &audit, true).await {
            Ok(started) => Tick::Started(started.how),
            Err(error) => {
                // A download that started in between is no failure: the next quiet moment tries
                // again.
                tracing::warn!(code = error.code(), error = %error.message(), "the automatic restart did not start");
                Tick::Refused(error.code().to_owned())
            }
        }
    }
}
