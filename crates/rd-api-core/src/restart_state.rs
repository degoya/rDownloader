//! A pending restart and the restart itself, as the service keeps them (RD-1240-32).
//!
//! **Pending.** A plugin version installed, updated, chosen or put under test runs only from the
//! next start; the plugin manager derives that from what this start loaded
//! (`rd_api_admin::restart_service` reads it there, so it never drifts). What it cannot derive --
//! a plugin switched on or off or removed, a key or a package withdrawn -- the route that did it
//! records here ([`RestartState::record`]). In memory: a restart is exactly what clears it.
//! What the derivation finds right at the start -- a plugin that fails to load at every start --
//! is kept as the [baseline](RestartState::remember_baseline): a restart would not change it, so
//! it is not pending, and the automatic restart never loops over it.
//!
//! **The restart.** How it happens is `rd_update::restart`'s decision from the install kind and
//! the [`Environment`] read at the start. [`RestartState::begin`] marks it once, so two requests
//! never both go ahead; the relauncher is started through [`RestartState::relaunch`], and a
//! restart left to a supervisor makes `serve` end with [`RestartState::exit_code`] after the
//! ordinary stop.

use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Utc};
use rd_update::restart::{Environment, RestartHow, RestartPlan, Supervisor, decide_how};
use rd_update::{InstallKind, RESTART_EXIT_CODE};

use crate::dto::RestartReason;

/// Starts the relauncher for a written plan.
pub type Relauncher = Arc<dyn Fn(&RestartPlan) -> anyhow::Result<()> + Send + Sync>;

/// The pending reasons the routes recorded and the restart under way, shared by every clone.
#[derive(Clone)]
pub struct RestartState(Arc<Inner>);

struct Inner {
    started_at: DateTime<Utc>,
    environment: RwLock<Environment>,
    relauncher: RwLock<Relauncher>,
    recorded: Mutex<Vec<RestartReason>>,
    /// What the derivation found at the start; `None` until it was read.
    baseline: Mutex<Option<Vec<RestartReason>>>,
    /// The way of the restart that began, once one did.
    requested: Mutex<Option<RestartHow>>,
}

impl Default for RestartState {
    fn default() -> Self {
        let relauncher: Relauncher = Arc::new(rd_update::restart::launch_relauncher);
        Self(Arc::new(Inner {
            started_at: Utc::now(),
            environment: RwLock::new(Environment::detect()),
            relauncher: RwLock::new(relauncher),
            recorded: Mutex::new(Vec::new()),
            baseline: Mutex::new(None),
            requested: Mutex::new(None),
        }))
    }
}

/// The two codes that undo each other: switching a plugin on after switching it off.
fn opposite(code: &str) -> Option<&'static str> {
    match code {
        "plugin_enabled" => Some("plugin_disabled"),
        "plugin_disabled" => Some("plugin_enabled"),
        "plugin_digest_revoked" => Some("plugin_digest_unrevoked"),
        "plugin_digest_unrevoked" => Some("plugin_digest_revoked"),
        _ => None,
    }
}

impl RestartState {
    /// Reads the environment as `environment` says and starts the relauncher with `relauncher`.
    ///
    /// For tests only, which must not start a process; the service never calls it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn use_environment(&self, environment: Environment, relauncher: Relauncher) {
        if let Ok(mut current) = self.0.environment.write() {
            *current = environment;
        }
        if let Ok(mut current) = self.0.relauncher.write() {
            *current = relauncher;
        }
    }

    /// When this process started.
    #[must_use]
    pub fn started_at(&self) -> DateTime<Utc> {
        self.0.started_at
    }

    /// How an installation of `kind` restarts here.
    #[must_use]
    pub fn how(&self, kind: InstallKind) -> (RestartHow, Option<Supervisor>) {
        let environment = self
            .0
            .environment
            .read()
            .map(|environment| *environment)
            .unwrap_or_default();
        decide_how(kind, environment)
    }

    /// Records a reason that waits for the next start. The same reason twice is kept once; a
    /// reason that undoes an earlier one (switched on after switched off) takes its place.
    pub fn record(&self, reason: RestartReason) {
        tracing::info!(code = %reason.code, plugin = reason.plugin_id.as_deref().unwrap_or(""), "a restart is pending");
        if let Ok(mut recorded) = self.0.recorded.lock() {
            let undone = opposite(&reason.code);
            recorded.retain(|earlier| {
                let same_subject =
                    earlier.plugin_id == reason.plugin_id && earlier.name == reason.name;
                !(same_subject
                    && (earlier.code == reason.code || Some(earlier.code.as_str()) == undone))
            });
            recorded.push(reason);
        }
    }

    /// The reasons recorded since the start, oldest first.
    #[must_use]
    pub fn recorded(&self) -> Vec<RestartReason> {
        self.0
            .recorded
            .lock()
            .map(|recorded| recorded.clone())
            .unwrap_or_default()
    }

    /// Keeps `reasons`, derived at the start, as what a restart does not change; only the first
    /// call counts.
    pub fn remember_baseline(&self, reasons: Vec<RestartReason>) {
        if let Ok(mut baseline) = self.0.baseline.lock()
            && baseline.is_none()
        {
            *baseline = Some(reasons);
        }
    }

    /// Whether `reason` was there at the start already.
    #[must_use]
    pub fn in_baseline(&self, reason: &RestartReason) -> bool {
        self.0.baseline.lock().is_ok_and(|baseline| {
            baseline
                .as_ref()
                .is_some_and(|baseline| baseline.contains(reason))
        })
    }

    /// Whether a restart began and the service is on its way down.
    #[must_use]
    pub fn restarting(&self) -> bool {
        self.0
            .requested
            .lock()
            .map(|requested| requested.is_some())
            .unwrap_or(true)
    }

    /// Marks a restart in `how` as begun; `false` when one began already.
    #[must_use]
    pub fn begin(&self, how: RestartHow) -> bool {
        let Ok(mut requested) = self.0.requested.lock() else {
            return false;
        };
        if requested.is_some() {
            return false;
        }
        *requested = Some(how);
        true
    }

    /// Takes back a restart that could not be started (the relauncher did not start).
    pub fn abandon(&self) {
        if let Ok(mut requested) = self.0.requested.lock() {
            *requested = None;
        }
    }

    /// Starts the relauncher for `plan`.
    ///
    /// # Errors
    ///
    /// When the plan, the copy or the start fails.
    pub fn relaunch(&self, plan: &RestartPlan) -> anyhow::Result<()> {
        let relauncher = self
            .0
            .relauncher
            .read()
            .map(|relauncher| Arc::clone(&relauncher))
            .map_err(|_| anyhow::anyhow!("restart relauncher lock poisoned"))?;
        relauncher(plan)
    }

    /// The code `serve` ends with: [`RESTART_EXIT_CODE`] when a restart began that a supervisor
    /// (or a person) carries out, `None` otherwise -- the relauncher's restart ends as any stop.
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        let requested = self
            .0
            .requested
            .lock()
            .ok()
            .and_then(|requested| *requested);
        match requested {
            Some(RestartHow::Supervisor | RestartHow::Manual) => Some(RESTART_EXIT_CODE),
            Some(RestartHow::Relaunch) | None => None,
        }
    }
}

#[cfg(test)]
#[path = "restart_state_tests.rs"]
mod tests;
