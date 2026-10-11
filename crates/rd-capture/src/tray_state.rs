//! The state the tray shows, without the tray.
//!
//! Which mark the icon carries, whether "Open rDownloader" can be chosen, which queue entries the
//! menu offers, what the two status items and the tooltip say: all of that is decided here, from the service state, the transfer poll
//! and the agent's notices about itself. Nothing in this module names a type from `png`, `tao`
//! or `tray-icon` -- those crates are Windows- and macOS-only, and the Linux agent must link
//! none of them -- so the rules compile and are tested on every host. `tray.rs` holds the
//! handles and applies what this module decides; `icon.rs` and `status.rs` are the same cut,
//! one step earlier than the obvious type (RD-109-13).
//!
//! The module carries the tray's gate plus `test`, like `status.rs`: the tests run here, and a
//! Linux release build compiles no code it cannot reach.
//!
//! Two rules a description of the whole surface would hide are stated as updates instead: the
//! icon is only named when it actually changes, because redrawing it on every poll flickers on
//! Windows; and a server-state change rewrites the server item but leaves the tooltip as it was,
//! which is what the tray did before its rules moved here and is not this module's to change.
//!
//! The menu has two status items since RD-1240-06: the agent's, with its address, the transfers
//! and its notices, and the server's, with its version and state. The tooltip has room for one
//! line only, so it keeps the combined line it always had and still names the server's state.
//!
//! "Pause while gaming" (RD-1240-23) is decided here too: ticked while game mode is switched on,
//! and chosen only by an agent that may control the queue, since its switch needs that right.
//!
//! While the service installs an update the server line says so instead of its version and state
//! (RD-1240-25): the service goes away while it restarts, and "not reachable" would be the wrong
//! story.

use rd_core::{CaptureAgentSettings, CaptureGameMode, CaptureShortcuts};
use url::Url;

use crate::{
    activity::{self, Activity, QueueMenu},
    game_mode,
    status::{
        ServerStatus, agent_label, server_label, server_restart_label, server_updating_label,
        status_label,
    },
    supervision::{AgentNotice, notice_label},
};

/// Which of the marks the tray shows.
///
/// The busy mark is the idle one with the activity badge painted over it, and the paused ones are
/// the same two greyed out while clipboard watching is paused (`icon.rs`, RD-1180-01); which is
/// up is decided here, what they look like is not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IconKind {
    Idle,
    Busy,
    PausedIdle,
    PausedBusy,
}

/// Everything the surface shows, as the state wants it now: what a tray built at this moment is
/// given, and what a tray that has applied every update since it was built is showing. The two
/// are the same thing, and a test holds them together.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Surface {
    pub icon: IconKind,
    pub open_enabled: bool,
    /// The queue entries: none, greyed out with the pairing hint, or which of them can be chosen
    /// (RD-1100-06, RD-1101-06).
    pub queue: QueueMenu,
    /// The agent's line: version, address, transfers, notices.
    pub status_line: String,
    /// The server's line under it: version and state (RD-1240-06).
    pub server_line: String,
    pub tooltip: String,
    /// The check mark of "Pause clipboard watching" (RD-1180-01).
    pub clipboard_paused: bool,
    /// The shortcuts the menu shows beside its entries (RD-1180-03).
    pub accelerators: CaptureShortcuts,
    /// "Pause while gaming" (RD-1240-23).
    pub game_mode: GameModeEntry,
}

/// "Pause while gaming": its label, its check mark and why it cannot be chosen, if it cannot
/// (RD-1240-23). A shortcut for it obeys the same rule and logs the reason.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct GameModeEntry {
    pub label: &'static str,
    pub checked: bool,
    pub refused: Option<&'static str>,
}

/// The entry's label while game mode watches for something. English, like the rest of the menu
/// (RD-092-05).
pub(crate) const GAME_MODE: &str = "Pause while gaming";
/// The label while nothing is set to watch for: there is nothing to switch until the settings
/// page names a program or full screen, and the greyed entry says where.
pub(crate) const GAME_MODE_UNSET: &str = "Pause while gaming (set up in Settings)";

impl GameModeEntry {
    /// The entry for these settings and this queue reading: greyed out with a hint while nothing
    /// is set up, greyed out for an agent that may not control the queue or before the service
    /// said whether it may (the hint about pairing stands among the queue entries).
    pub(crate) fn new(mode: &CaptureGameMode, queue: QueueMenu) -> Self {
        if !mode.watches() {
            return Self {
                label: GAME_MODE_UNSET,
                checked: false,
                refused: Some(game_mode::NOTHING_SET_UP),
            };
        }
        Self {
            label: GAME_MODE,
            checked: mode.enabled,
            refused: match queue {
                QueueMenu::Offered(_) => None,
                QueueMenu::Locked => Some(game_mode::NO_QUEUE_CONTROL),
                QueueMenu::Hidden => Some(game_mode::QUEUE_CONTROL_UNKNOWN),
            },
        }
    }

    /// Whether the entry, and a shortcut for it, can be chosen.
    pub(crate) fn enabled(self) -> bool {
        self.refused.is_none()
    }
}

/// What one event changes on the surface. A field left `None` is left alone.
///
/// Not a whole [`Surface`], because which parts an event touches is itself a rule: the icon is
/// only redrawn on a real change, and "Open" only moves with the service state.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct Update {
    pub icon: Option<IconKind>,
    pub open_enabled: Option<bool>,
    /// Named only on a change: swapping the entries rebuilds that part of the menu.
    pub queue: Option<QueueMenu>,
    pub status_line: Option<String>,
    pub server_line: Option<String>,
    pub tooltip: Option<String>,
    pub clipboard_paused: Option<bool>,
    /// Named only on a change: every entry's accelerator is written again.
    pub accelerators: Option<CaptureShortcuts>,
    /// Named only on a change, from the settings or the queue reading.
    pub game_mode: Option<GameModeEntry>,
}

impl Update {
    /// Nothing to apply: the event said what the tray already shows.
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The tray's state machine: what has been seen, and what the surface should show for it.
#[derive(Clone, Debug)]
pub(crate) struct TrayState {
    busy: bool,
    /// The queue entries the last transfer reading allowed; none until the first one.
    queue: QueueMenu,
    /// The transfer line, appended to the server line; `None` until the first reading.
    transfers: Option<String>,
    /// What the agent has to say about itself, appended after the transfers; `None` while there
    /// is nothing wrong. Only the latest is kept: the status item is one line.
    notice: Option<String>,
    /// The head of the agent's line: product, version, address.
    agent: String,
    /// The head of the tooltip, as `status_label` renders it for the current state.
    status: String,
    /// The configured service, or `None` when the agent is not paired; only used for relabelling.
    configured: Option<Url>,
    server: ServerStatus,
    /// The version the service's health answer named, while it is running.
    server_version: Option<String>,
    /// The version the service is being updated to, while it is (RD-1240-25).
    server_update: Option<String>,
    /// A restart of the service is pending (RD-1240-32).
    server_restart: bool,
    /// What the tooltip says now: the product name until the first transfer reading or notice,
    /// then the cut status line of whichever came last.
    tooltip: String,
    /// What the service has the agent set to; the defaults until the first reading.
    settings: CaptureAgentSettings,
}

/// The tooltip a freshly built tray carries, before anything has been read.
pub(crate) fn initial_tooltip() -> String {
    format!("rDownloader Capture v{}", env!("CARGO_PKG_VERSION"))
}

impl TrayState {
    /// Before anything has answered: the service is starting, nothing transfers.
    pub(crate) fn new(configured: Option<Url>) -> Self {
        let server = ServerStatus::Starting;
        Self {
            busy: false,
            queue: QueueMenu::Hidden,
            transfers: None,
            notice: None,
            agent: agent_label(configured.as_ref()),
            status: status_label(configured.as_ref(), server),
            configured,
            server,
            server_version: None,
            server_update: None,
            server_restart: false,
            tooltip: initial_tooltip(),
            settings: CaptureAgentSettings::default(),
        }
    }

    /// Everything the surface should show right now.
    pub(crate) fn surface(&self) -> Surface {
        Surface {
            icon: self.icon(),
            open_enabled: open_enabled(self.server),
            queue: self.queue,
            status_line: self.status_line(),
            server_line: self.server_line(),
            tooltip: self.tooltip.clone(),
            clipboard_paused: self.settings.clipboard_paused,
            accelerators: self.settings.shortcuts.clone(),
            game_mode: self.game_mode(),
        }
    }

    fn game_mode(&self) -> GameModeEntry {
        GameModeEntry::new(&self.settings.game_mode, self.queue)
    }

    /// Rewrites the server line and gates "Open rDownloader" on the service answering.
    ///
    /// Opening the browser while the service is still starting lands on an error page, and the
    /// user has no way to tell that from the service being down. A status the tray already
    /// holds changes nothing: the health poll sends on every tick. `version` is what the
    /// health answer named; it is shown only while the service runs, since a service that does
    /// not answer has no version to report (RD-1240-06).
    pub(crate) fn on_server_status(
        &mut self,
        status: ServerStatus,
        version: Option<String>,
    ) -> Update {
        let version = version.filter(|_| status == ServerStatus::Running);
        if self.server == status && self.server_version == version {
            return Update::default();
        }
        self.server = status;
        self.server_version = version;
        self.status = status_label(self.configured.as_ref(), status);
        Update {
            open_enabled: Some(open_enabled(status)),
            server_line: Some(self.server_line()),
            ..Update::default()
        }
    }

    /// Follows an install of the service's update (RD-1240-25): the server line names the version
    /// while it runs and goes back to the service's state once it ended.
    pub(crate) fn on_server_update(
        &mut self,
        updating: Option<String>,
        restart_pending: bool,
    ) -> Update {
        if self.server_update == updating && self.server_restart == restart_pending {
            return Update::default();
        }
        self.server_update = updating;
        self.server_restart = restart_pending;
        Update {
            server_line: Some(self.server_line()),
            ..Update::default()
        }
    }

    /// Swaps the mark when transfers start or stop, and keeps the status line current.
    ///
    /// The icon is only named on an actual change: `set_icon` redraws the tray, and doing that
    /// every five seconds makes it flicker on Windows for no reason.
    ///
    /// The queue entries follow the same reading, and are only named when they change: the
    /// menu is rebuilt around them, and doing that every five seconds would close a menu the
    /// person has open.
    pub(crate) fn on_transfers(&mut self, activity: Activity) -> Update {
        let game_mode = self.game_mode();
        let changed = self.busy != activity.running;
        self.busy = activity.running;
        let queue_changed = self.queue != activity.queue;
        self.queue = activity.queue;
        self.transfers = Some(activity.detail);
        let line = self.status_line();
        // The menu item takes the line whole; Windows keeps a tooltip in a fixed buffer and
        // silently loses whatever runs past it, so that one is cut to fit.
        self.tooltip = self.tooltip_text();
        Update {
            icon: changed.then_some(self.icon()),
            open_enabled: None,
            queue: queue_changed.then_some(self.queue),
            status_line: Some(line),
            tooltip: Some(self.tooltip.clone()),
            game_mode: (self.game_mode() != game_mode).then(|| self.game_mode()),
            ..Update::default()
        }
    }

    /// Reports something the agent noticed about itself.
    ///
    /// The tray used to keep saying "healthy" after a background task had died or after
    /// Click'n'Load had got only half its addresses, which is precisely when the agent has
    /// stopped doing what it claims (RD-109-07). The same notice twice is nothing new.
    pub(crate) fn on_notice(&mut self, notice: &AgentNotice) -> Update {
        let label = notice_label(notice);
        if self.notice.as_deref() == Some(label.as_str()) {
            return Update::default();
        }
        self.notice = Some(label);
        let line = self.status_line();
        self.tooltip = self.tooltip_text();
        Update {
            status_line: Some(line),
            tooltip: Some(self.tooltip.clone()),
            ..Update::default()
        }
    }

    /// Follows the clipboard pause, the shortcuts and game mode (RD-1180-01, RD-1180-03,
    /// RD-1240-23).
    ///
    /// The pause greys the mark, ticks its entry and says so on the status line and in the
    /// tooltip, which is how a paused agent is told apart at a glance; the shortcuts are only
    /// written again when they changed. A reading that changes nothing changes nothing: the
    /// settings arrive on every change only, but the first one may equal the defaults.
    pub(crate) fn on_settings(&mut self, settings: &CaptureAgentSettings) -> Update {
        let paused_changed = self.settings.clipboard_paused != settings.clipboard_paused;
        let shortcuts_changed = self.settings.shortcuts != settings.shortcuts;
        let before = self.icon();
        let game_mode = self.game_mode();
        self.settings = settings.clone();
        let mut update = Update {
            accelerators: shortcuts_changed.then(|| settings.shortcuts.clone()),
            game_mode: (self.game_mode() != game_mode).then(|| self.game_mode()),
            ..Update::default()
        };
        if paused_changed {
            let line = self.status_line();
            self.tooltip = self.tooltip_text();
            update.icon = (self.icon() != before).then_some(self.icon());
            update.clipboard_paused = Some(settings.clipboard_paused);
            update.status_line = Some(line);
            update.tooltip = Some(self.tooltip.clone());
        }
        update
    }

    fn icon(&self) -> IconKind {
        match (self.settings.clipboard_paused, self.busy) {
            (false, false) => IconKind::Idle,
            (false, true) => IconKind::Busy,
            (true, false) => IconKind::PausedIdle,
            (true, true) => IconKind::PausedBusy,
        }
    }

    /// The agent's status item: the agent and its address, the transfers when there are any,
    /// and whatever the agent has to say about itself.
    ///
    /// Composed in `activity`, which compiles everywhere, so the length this can reach is
    /// measured by tests that also run on the Linux hosts where no tray exists.
    fn status_line(&self) -> String {
        self.with_details(&self.agent)
    }

    /// The server's status item under it (RD-1240-06), or the update it installs (RD-1240-25).
    fn server_line(&self) -> String {
        match &self.server_update {
            Some(target) => server_updating_label(target),
            None if self.server_restart => {
                server_restart_label(self.server, self.server_version.as_deref())
            }
            None => server_label(self.server, self.server_version.as_deref()),
        }
    }

    /// The tooltip: the one line naming the agent, the server's state and the details, cut to
    /// what Windows can carry.
    fn tooltip_text(&self) -> String {
        activity::tooltip(&self.with_details(&self.status))
    }

    /// `head`, then the transfers, the notice and the clipboard pause, each when there is one.
    fn with_details(&self, head: &str) -> String {
        let line = activity::status_line(head, self.transfers.as_deref().unwrap_or_default());
        let line = activity::status_line(&line, self.notice.as_deref().unwrap_or_default());
        let paused = if self.settings.clipboard_paused {
            CLIPBOARD_PAUSED
        } else {
            ""
        };
        activity::status_line(&line, paused)
    }
}

/// What the status line and the tooltip add while clipboard watching is paused. English, like the
/// rest of the menu (RD-092-05).
pub(crate) const CLIPBOARD_PAUSED: &str = "capture paused";

/// "Open rDownloader" can be chosen exactly while the service answers.
fn open_enabled(server: ServerStatus) -> bool {
    server == ServerStatus::Running
}

#[cfg(test)]
#[path = "tray_state_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tray_state_server_tests.rs"]
mod server_tests;

#[cfg(test)]
#[path = "tray_state_game_mode_tests.rs"]
mod game_mode_tests;
