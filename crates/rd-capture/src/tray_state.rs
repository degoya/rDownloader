//! The state the tray shows, without the tray.
//!
//! Which mark the icon carries, whether "Open rDownloader" can be chosen, which queue entries the
//! menu offers, what the status item and the tooltip say: all of that is decided here, from the service state, the transfer poll
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
//! Windows; and a server-state change rewrites the status item but leaves the tooltip as it was,
//! which is what the tray did before its rules moved here and is not this module's to change.

use url::Url;

use crate::{
    activity::{self, Activity, QueueMenu},
    status::{ServerStatus, status_label},
    supervision::{AgentNotice, notice_label},
};

/// Which of the two marks the tray shows.
///
/// The busy mark is the idle one with the activity badge painted over it (`icon.rs`); which of
/// the two is up is decided here, what they look like is not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IconKind {
    Idle,
    Busy,
}

/// Everything the surface shows, as the state wants it now: what a tray built at this moment is
/// given, and what a tray that has applied every update since it was built is showing. The two
/// are the same thing, and a test holds them together.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Surface {
    pub icon: IconKind,
    pub open_enabled: bool,
    /// The queue entries: none, "pause", or "resume" (RD-1100-06).
    pub queue: QueueMenu,
    pub status_line: String,
    pub tooltip: String,
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
    pub tooltip: Option<String>,
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
    /// The server line, as `status_label` renders it for the current state.
    status: String,
    /// The configured service, or `None` when the agent is not paired; only used for relabelling.
    configured: Option<Url>,
    server: ServerStatus,
    /// What the tooltip says now: the product name until the first transfer reading or notice,
    /// then the cut status line of whichever came last.
    tooltip: String,
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
            status: status_label(configured.as_ref(), server),
            configured,
            server,
            tooltip: initial_tooltip(),
        }
    }

    /// Everything the surface should show right now.
    pub(crate) fn surface(&self) -> Surface {
        Surface {
            icon: self.icon(),
            open_enabled: open_enabled(self.server),
            queue: self.queue,
            status_line: self.status_line(),
            tooltip: self.tooltip.clone(),
        }
    }

    /// Rewrites the status line and gates "Open rDownloader" on the service answering.
    ///
    /// Opening the browser while the service is still starting lands on an error page, and the
    /// user has no way to tell that from the service being down. A status the tray already
    /// holds changes nothing: the health poll sends on every tick.
    pub(crate) fn on_server_status(&mut self, status: ServerStatus) -> Update {
        if self.server == status {
            return Update::default();
        }
        self.server = status;
        self.status = status_label(self.configured.as_ref(), status);
        Update {
            open_enabled: Some(open_enabled(status)),
            status_line: Some(self.status_line()),
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
        let changed = self.busy != activity.running;
        self.busy = activity.running;
        let queue_changed = self.queue != activity.queue;
        self.queue = activity.queue;
        self.transfers = Some(activity.detail);
        let line = self.status_line();
        // The menu item takes the line whole; Windows keeps a tooltip in a fixed buffer and
        // silently loses whatever runs past it, so that one is cut to fit.
        self.tooltip = activity::tooltip(&line);
        Update {
            icon: changed.then_some(self.icon()),
            open_enabled: None,
            queue: queue_changed.then_some(self.queue),
            status_line: Some(line),
            tooltip: Some(self.tooltip.clone()),
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
        self.tooltip = activity::tooltip(&line);
        Update {
            status_line: Some(line),
            tooltip: Some(self.tooltip.clone()),
            ..Update::default()
        }
    }

    fn icon(&self) -> IconKind {
        if self.busy {
            IconKind::Busy
        } else {
            IconKind::Idle
        }
    }

    /// The status item's text: the server line, the transfers when there are any, and whatever
    /// the agent has to say about itself.
    ///
    /// Composed in `activity`, which compiles everywhere, so the length this can reach is
    /// measured by tests that also run on the Linux hosts where no tray exists.
    fn status_line(&self) -> String {
        let line =
            activity::status_line(&self.status, self.transfers.as_deref().unwrap_or_default());
        activity::status_line(&line, self.notice.as_deref().unwrap_or_default())
    }
}

/// "Open rDownloader" can be chosen exactly while the service answers.
fn open_enabled(server: ServerStatus) -> bool {
    server == ServerStatus::Running
}

#[cfg(test)]
#[path = "tray_state_tests.rs"]
mod tests;
