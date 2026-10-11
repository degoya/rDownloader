//! Installing an offered update by itself (RD-1240-27): when, not how.
//!
//! The switch is off by default and does nothing where the installation does not install itself
//! (`InstallKind::installs_itself`: a package manager, a container). Where it does, the offered
//! version is installed through the same path as the click in the interface, and only at a
//! moment nobody notices: nothing has run for [`QUIET_PERIOD`] (no transfer, no post-processing,
//! no recording), inside the optional time window, and never twice for a version whose install
//! already failed or was taken back. The caller observes what runs and asks; this decides.
//!
//! The quiet half -- nothing running for [`QUIET_PERIOD`], inside the window -- is [`settled`]
//! over the clock [`AutoInstall::observe`] keeps, which the automatic restart (RD-1240-32,
//! `crate::auto_restart`) decides with too.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// How long nothing may have run before an update is installed.
pub const QUIET_PERIOD: TimeDelta = TimeDelta::minutes(5);
/// Minutes in a day; a window's bounds lie below it.
pub const MINUTES_PER_DAY: u16 = 24 * 60;

/// The time of day an automatic install may start, in the installation's own time zone.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstallWindow {
    /// Minutes after midnight, inclusive.
    pub start_minute: u16,
    /// Minutes after midnight, exclusive; below `start_minute` the window wraps past midnight.
    pub end_minute: u16,
}

impl InstallWindow {
    /// Both bounds within a day and not the same: an empty window would never install.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.start_minute < MINUTES_PER_DAY
            && self.end_minute < MINUTES_PER_DAY
            && self.start_minute != self.end_minute
    }

    /// Whether `minute` (after local midnight) lies inside.
    #[must_use]
    pub fn covers(self, minute: u16) -> bool {
        if self.start_minute < self.end_minute {
            (self.start_minute..self.end_minute).contains(&minute)
        } else {
            minute >= self.start_minute || minute < self.end_minute
        }
    }
}

/// What the caller sees at one moment.
#[derive(Clone, Copy, Debug)]
pub struct Moment<'a> {
    /// The switch.
    pub enabled: bool,
    /// `InstallKind::installs_itself` and a program folder to replace.
    pub installs_itself: bool,
    /// The version offered for an install on the channel in force, if any.
    pub offered: Option<&'a str>,
    /// An install is under way, from the first step to the updater's last.
    pub installing: bool,
    /// The last install of the offered version failed or was taken back.
    pub failed_before: bool,
    /// A transfer, a post-processing step or a recording runs.
    pub busy: bool,
    /// Minutes after local midnight.
    pub local_minute: u16,
    pub window: Option<InstallWindow>,
}

/// Why nothing is installed now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wait {
    /// The switch is off.
    Off,
    /// This installation does not install itself.
    Unsupported,
    /// No newer version is offered for an install.
    NothingOffered,
    /// No restart is pending (the automatic restart, RD-1240-32).
    NothingPending,
    /// An install runs already; for the automatic restart, an install or a restart.
    Installing,
    /// The offered version failed or was taken back once; it waits for a person.
    FailedBefore,
    /// Something runs.
    Busy,
    /// Nothing runs, but not yet for [`QUIET_PERIOD`].
    Settling,
    /// Outside the time window.
    OutsideWindow,
}

impl Wait {
    /// The stable name, for the log.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Unsupported => "unsupported",
            Self::NothingOffered => "nothing_offered",
            Self::NothingPending => "nothing_pending",
            Self::Installing => "installing",
            Self::FailedBefore => "failed_before",
            Self::Busy => "busy",
            Self::Settling => "settling",
            Self::OutsideWindow => "outside_window",
        }
    }
}

/// Follows how long nothing has run, and decides at each moment.
#[derive(Clone, Debug, Default)]
pub struct AutoInstall {
    /// Since when nothing runs; `None` while something does.
    quiet_since: Option<DateTime<Utc>>,
}

impl AutoInstall {
    /// Observes `moment` at `now` and answers the version to install, or why not.
    ///
    /// Whatever the answer, the quiet clock follows `busy`, so switching the setting on after an
    /// idle night installs at once rather than five minutes later.
    ///
    /// # Errors
    ///
    /// The [`Wait`] that holds the install back.
    pub fn decide<'a>(&mut self, moment: &Moment<'a>, now: DateTime<Utc>) -> Result<&'a str, Wait> {
        let quiet_since = self.observe(moment.busy, now);
        if !moment.enabled {
            return Err(Wait::Off);
        }
        if !moment.installs_itself {
            return Err(Wait::Unsupported);
        }
        let Some(version) = moment.offered else {
            return Err(Wait::NothingOffered);
        };
        if moment.installing {
            return Err(Wait::Installing);
        }
        if moment.failed_before {
            return Err(Wait::FailedBefore);
        }
        settled(quiet_since, now, moment.local_minute, moment.window)?;
        Ok(version)
    }

    /// Follows `busy` at `now`: since when nothing runs, `None` while something does.
    pub fn observe(&mut self, busy: bool, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        if busy {
            self.quiet_since = None;
            None
        } else {
            Some(*self.quiet_since.get_or_insert(now))
        }
    }
}

/// The quiet half of a decision: nothing has run since `quiet_since` for [`QUIET_PERIOD`], and
/// `local_minute` lies inside the window, if there is one.
///
/// # Errors
///
/// [`Wait::Busy`], [`Wait::Settling`] or [`Wait::OutsideWindow`].
pub fn settled(
    quiet_since: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    local_minute: u16,
    window: Option<InstallWindow>,
) -> Result<(), Wait> {
    let Some(since) = quiet_since else {
        return Err(Wait::Busy);
    };
    if now - since < QUIET_PERIOD {
        return Err(Wait::Settling);
    }
    if window.is_some_and(|window| !window.covers(local_minute)) {
        return Err(Wait::OutsideWindow);
    }
    Ok(())
}

#[cfg(test)]
#[path = "auto_install_tests.rs"]
mod tests;
