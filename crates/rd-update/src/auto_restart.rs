//! Restarting the service by itself when a restart is pending (RD-1240-32): when, not how.
//!
//! The switch (`restart_when_needed`) is off by default. Switched on, a pending restart -- a
//! plugin installed or updated, by hand or by the automatic plugin update, that runs only from
//! the next start -- is carried out at the moment an automatic install would be: nothing has run
//! for [`QUIET_PERIOD`](crate::auto_install::QUIET_PERIOD) (no transfer, no post-processing, no
//! recording), inside the optional time window the automatic install uses as well, and never
//! while an update is being installed or a restart runs already. The quiet clock is the automatic
//! install's own ([`AutoInstall::observe`], [`settled`]); the caller observes and asks, this
//! decides.

use chrono::{DateTime, Utc};

use crate::auto_install::{AutoInstall, InstallWindow, Wait, settled};

/// What the caller sees at one moment.
#[derive(Clone, Copy, Debug)]
pub struct RestartMoment {
    /// The switch.
    pub enabled: bool,
    /// A restart is pending.
    pub pending: bool,
    /// A restart can begin now: no update is being installed, none runs already.
    pub can_restart: bool,
    /// A transfer, a post-processing step or a recording runs.
    pub busy: bool,
    /// Minutes after local midnight.
    pub local_minute: u16,
    pub window: Option<InstallWindow>,
}

/// Follows how long nothing has run, and decides at each moment.
#[derive(Clone, Debug, Default)]
pub struct AutoRestart {
    clock: AutoInstall,
}

impl AutoRestart {
    /// Observes `moment` at `now` and answers whether to restart now, or why not.
    ///
    /// Whatever the answer, the quiet clock follows `busy`, as the automatic install's does.
    ///
    /// # Errors
    ///
    /// The [`Wait`] that holds the restart back: [`Wait::Off`], [`Wait::NothingPending`],
    /// [`Wait::Installing`] (an update or a restart runs), [`Wait::Busy`], [`Wait::Settling`] or
    /// [`Wait::OutsideWindow`].
    pub fn decide(&mut self, moment: &RestartMoment, now: DateTime<Utc>) -> Result<(), Wait> {
        let quiet_since = self.clock.observe(moment.busy, now);
        if !moment.enabled {
            return Err(Wait::Off);
        }
        if !moment.pending {
            return Err(Wait::NothingPending);
        }
        if !moment.can_restart {
            return Err(Wait::Installing);
        }
        settled(quiet_since, now, moment.local_minute, moment.window)
    }
}

#[cfg(test)]
#[path = "auto_restart_tests.rs"]
mod tests;
