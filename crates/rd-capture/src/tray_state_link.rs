//! What the tray shows about the link (RD-1390-02), apart from `tray_state.rs` so the rules stay
//! in one place.
//!
//! Unlinked, the agent's line says "Not linked", the mark is the quiet greyed one rather than an
//! error, and everything that needs a service is greyed out: "Open rDownloader", "About
//! rDownloader", the clipboard entries, "Pause while gaming"; the queue entries are not shown.
//! "Link capture agent", "About" and "Quit" stay. Linked, the same entry reads "Change link…",
//! and a link over plain `http` to another machine says "(unencrypted)" on the server line, so
//! nobody forgets it was chosen. The words are English, like the rest of the menu (RD-092-05).

use super::{Surface, TrayState, Update};
use crate::{
    activity::QueueMenu,
    link_phase::LinkStatus,
    status::{ServerStatus, agent_label, status_label},
};

/// The entry while the agent is not linked.
pub(crate) const LINK: &str = "Link capture agent";
/// The same entry once it is.
pub(crate) const CHANGE_LINK: &str = "Change link\u{2026}";
/// The agent's line while it is not linked.
pub(crate) const NOT_LINKED: &str = "Not linked";
/// What the server line adds for a link over plain `http` to another machine.
pub(crate) const UNENCRYPTED: &str = "(unencrypted)";

/// The link entry, and whether the entries that need a service can be chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct LinkEntry {
    pub label: &'static str,
    pub linked: bool,
}

impl TrayState {
    /// The agent said whether it is linked, and with what. A link to another service starts the
    /// server's half over: what the old one said no longer holds.
    pub(crate) fn on_link(&mut self, status: &LinkStatus) -> Update {
        let before = self.surface();
        match status {
            LinkStatus::Unlinked => {
                self.unlinked = true;
                self.insecure = false;
                self.forget_the_service();
                self.agent = format!(
                    "rDownloader Capture v{} \u{2014} {NOT_LINKED}",
                    env!("CARGO_PKG_VERSION")
                );
                self.status = self.agent.clone();
            }
            LinkStatus::Linked { service, insecure } => {
                if self.configured.as_ref() != Some(service) {
                    self.forget_the_service();
                    self.server = ServerStatus::Starting;
                    self.server_version = None;
                }
                self.unlinked = false;
                self.insecure = *insecure;
                self.configured = Some(service.clone());
                self.agent = agent_label(self.configured.as_ref());
                self.status = status_label(self.configured.as_ref(), self.server);
            }
        }
        if self.surface() != before {
            self.tooltip = self.tooltip_text();
        }
        self.changes_since(&before)
    }

    /// The readings of a service the agent no longer talks to.
    fn forget_the_service(&mut self) {
        self.busy = false;
        self.queue = QueueMenu::Hidden;
        self.transfers = None;
        self.server_update = None;
        self.server_restart = false;
    }

    /// "Open rDownloader" and "About rDownloader" can be chosen exactly while the agent is linked
    /// and the service answers.
    pub(super) fn open_enabled(&self) -> bool {
        !self.unlinked && self.server == ServerStatus::Running
    }

    pub(super) fn link_entry(&self) -> LinkEntry {
        let label = if self.unlinked || self.configured.is_none() {
            LINK
        } else {
            CHANGE_LINK
        };
        LinkEntry {
            label,
            linked: !self.unlinked,
        }
    }

    /// The server line, with "(unencrypted)" for a link that is.
    pub(super) fn with_transport(&self, line: String) -> String {
        if self.insecure && !self.unlinked {
            format!("{line} {UNENCRYPTED}")
        } else {
            line
        }
    }

    /// Everything that differs from `before`, and nothing else: an event that changes nothing
    /// redraws nothing.
    fn changes_since(&self, before: &Surface) -> Update {
        let now = self.surface();
        Update {
            icon: (now.icon != before.icon).then_some(now.icon),
            open_enabled: (now.open_enabled != before.open_enabled).then_some(now.open_enabled),
            queue: (now.queue != before.queue).then_some(now.queue),
            status_line: (now.status_line != before.status_line).then(|| now.status_line.clone()),
            server_line: (now.server_line != before.server_line).then(|| now.server_line.clone()),
            tooltip: (now.tooltip != before.tooltip).then(|| now.tooltip.clone()),
            game_mode: (now.game_mode != before.game_mode).then_some(now.game_mode),
            link: (now.link != before.link).then_some(now.link),
            ..Update::default()
        }
    }
}
