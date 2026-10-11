//! The agent's automatic install of its own update (RD-1240-27): the switch in
//! `self-update.json` (`rdownloader-capture update --auto-install on|off`, the tray's "Install
//! updates automatically"), off by default.
//!
//! Switched on, a check that finds a version this agent installs itself installs it at once, the
//! same way as the tray's "Install update to X": signature-checked, never older, taken back when
//! the new version does not start. An agent has nothing running that the restart would lose — a
//! transfer runs in the service — so it waits for no quiet moment. Beside the service the switch
//! does nothing and the tray does not show it: the service's own update replaces both programs,
//! and its setting decides.

use rd_update::agent::AgentSetup;
use rd_update::{InstallKind, Offer};

use super::{Config, OfferEntry};

/// What the tray and the shortcuts ask of the agent's own update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Request {
    /// "Install update to X", from the tray or a shortcut (RD-1240-24).
    Install,
    /// "Install updates automatically", switched.
    ToggleAutoInstall,
}

/// The tray's "Install updates automatically": whether it can be chosen, and its tick.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) struct AutoInstallEntry {
    /// Only where the agent installs itself; a package manager's agent shows it greyed out.
    pub(crate) enabled: bool,
    pub(crate) checked: bool,
}

/// What the tray shows of the agent's own update. Only the tray reads it, and Linux has none.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) struct UpdateMenu {
    /// "Install update to X", or how to get it; `None` while nothing is offered.
    pub(crate) offer: Option<OfferEntry>,
    /// `None` beside the service, whose setting decides.
    pub(crate) auto_install: Option<AutoInstallEntry>,
}

/// Whether this agent installs its own update itself: alone, from the portable archive
/// (`AgentSetup::action`).
fn installs_itself(setup: AgentSetup) -> bool {
    setup == AgentSetup::Alone(InstallKind::Portable)
}

/// The switch's entry for `setup`; see [`UpdateMenu::auto_install`].
pub(crate) fn auto_install_entry(setup: AgentSetup, settings: &Config) -> Option<AutoInstallEntry> {
    (setup != AgentSetup::WithService).then(|| {
        let enabled = installs_itself(setup);
        AutoInstallEntry {
            enabled,
            checked: enabled && settings.auto_install,
        }
    })
}

/// Whether a check that found `offer` installs it right away: switched on, and an offer this
/// agent installs itself, with its archive.
pub(crate) fn installs_now(setup: AgentSetup, settings: &Config, offer: Option<&Offer>) -> bool {
    settings.auto_install
        && installs_itself(setup)
        && offer.is_some_and(|offer| offer.artifact.is_some())
}

/// Switches the automatic install, where the switch applies. Returns whether it is now on.
///
/// # Errors
///
/// Why it does not apply here, for the log.
pub(crate) fn toggle(setup: AgentSetup, settings: &mut Config) -> Result<bool, &'static str> {
    if setup == AgentSetup::WithService {
        return Err("the service beside this agent updates it; its own setting decides");
    }
    if !installs_itself(setup) {
        return Err("this agent is updated by its package manager or by hand");
    }
    settings.auto_install = !settings.auto_install;
    Ok(settings.auto_install)
}
