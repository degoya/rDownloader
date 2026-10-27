//! Every entry of the tray menu and the command each one carries out (RD-1240-24).
//!
//! The tray's handle builds its items for these entries and turns a click into the entry's
//! command, which `controls::action` carries out the same way as the command's shortcut. Every
//! entry a click can choose has a command, so every tray function can be put on a shortcut
//! (owner, 2026-10-10; none is assigned by default). The tests below hold that for every entry
//! to come: the tray is compiled on Windows and macOS only, this list on every host.

use rd_core::CaptureCommand;

/// The title at the top of the menu, beside the logo: the product and the agent's version
/// (RD-1260-08).
pub(crate) fn title_line() -> String {
    format!("rDownloader v{}", env!("CARGO_PKG_VERSION"))
}

/// One entry of the tray menu, separators aside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuEntry {
    /// The title at the very top: the logo, "rDownloader" and the agent's version (RD-1260-08).
    Title,
    /// The agent's line under it (RD-1100-06).
    StatusLine,
    /// The server's line under it (RD-1240-06).
    ServerLine,
    /// "Link capture agent", "Change link…" once linked (RD-1390-02): one place under the two
    /// lines, its label following the link.
    Link,
    Open,
    /// "About rDownloader": the app's About page in the browser (RD-1260-08).
    About,
    StartAll,
    PauseAll,
    PauseHalfHour,
    PauseHour,
    /// "Add all from LinkGrabber" (RD-1240-07).
    AddAll,
    AddAllPaused,
    /// "Pair again to control the queue", under the greyed-out queue entries.
    PairHint,
    ClipboardWatch,
    SendClipboard,
    GameMode,
    /// "Start at login" (RD-1390-03), ticked while the agent is registered; hidden beside the
    /// service.
    Autostart,
    ServerUpdate,
    /// "Restart server", while a restart is pending (RD-1240-32).
    RestartServer,
    /// The agent's own "Install update to X" (RD-1210-03).
    Update,
    AutoInstall,
    Quit,
}

impl MenuEntry {
    /// Every entry, in the order of the menu.
    pub(crate) const ALL: [Self; 22] = [
        Self::Title,
        Self::StatusLine,
        Self::ServerLine,
        Self::Link,
        Self::Open,
        Self::About,
        Self::StartAll,
        Self::PauseAll,
        Self::PauseHalfHour,
        Self::PauseHour,
        Self::AddAll,
        Self::AddAllPaused,
        Self::PairHint,
        Self::ClipboardWatch,
        Self::SendClipboard,
        Self::GameMode,
        Self::Autostart,
        Self::ServerUpdate,
        Self::RestartServer,
        Self::Update,
        Self::AutoInstall,
        Self::Quit,
    ];

    /// Whether a click can ever choose the entry. The title, the two lines and the hint are only
    /// read: the tray builds them greyed out and never enables them. Every other entry can be
    /// chosen at least some of the time, and is a function a shortcut can reach.
    pub(crate) fn clickable(self) -> bool {
        match self {
            Self::Title | Self::StatusLine | Self::ServerLine | Self::PairHint => false,
            Self::Link
            | Self::Open
            | Self::About
            | Self::StartAll
            | Self::PauseAll
            | Self::PauseHalfHour
            | Self::PauseHour
            | Self::AddAll
            | Self::AddAllPaused
            | Self::ClipboardWatch
            | Self::SendClipboard
            | Self::GameMode
            | Self::Autostart
            | Self::ServerUpdate
            | Self::RestartServer
            | Self::Update
            | Self::AutoInstall
            | Self::Quit => true,
        }
    }

    /// The command a click on the entry carries out, the one its shortcut carries out too.
    ///
    /// "Quit" has one like every other entry; only its shortcut has no default, since quitting by
    /// accident is the one thing a stray key press should not reach (`CaptureShortcuts::quit`).
    pub(crate) fn command(self) -> Option<CaptureCommand> {
        Some(match self {
            Self::Title | Self::StatusLine | Self::ServerLine | Self::PairHint => return None,
            Self::Link => CaptureCommand::Link,
            Self::Open => CaptureCommand::Open,
            Self::About => CaptureCommand::About,
            Self::StartAll => CaptureCommand::StartAll,
            Self::PauseAll => CaptureCommand::PauseAll,
            Self::PauseHalfHour => CaptureCommand::PauseHalfHour,
            Self::PauseHour => CaptureCommand::PauseHour,
            Self::AddAll => CaptureCommand::AddAllFromLinkGrabber,
            Self::AddAllPaused => CaptureCommand::AddAllFromLinkGrabberPaused,
            Self::ClipboardWatch => CaptureCommand::ClipboardWatch,
            Self::SendClipboard => CaptureCommand::SendClipboard,
            Self::GameMode => CaptureCommand::GameMode,
            Self::Autostart => CaptureCommand::Autostart,
            Self::ServerUpdate => CaptureCommand::InstallServerUpdate,
            Self::RestartServer => CaptureCommand::RestartServer,
            Self::Update => CaptureCommand::InstallUpdate,
            Self::AutoInstall => CaptureCommand::AutoInstall,
            Self::Quit => CaptureCommand::Quit,
        })
    }
}

#[cfg(test)]
mod tests {
    use rd_core::CaptureCommand;

    use super::{MenuEntry, title_line};

    /// The ratchet (RD-1240-24): an entry a click can choose comes with its command, so it can be
    /// put on a shortcut. A new tray function without one fails here, not in somebody's hands.
    #[test]
    fn every_clickable_tray_entry_has_a_command() {
        for entry in MenuEntry::ALL {
            assert_eq!(
                entry.command().is_some(),
                entry.clickable(),
                "{entry:?}: a clickable entry needs a CaptureCommand, a read-only line has none"
            );
        }
    }

    /// Each command belongs to exactly one entry: none is offered in the settings without an
    /// entry in the menu, and no two entries share one.
    #[test]
    fn every_command_belongs_to_exactly_one_entry() {
        for command in CaptureCommand::ALL {
            let entries: Vec<MenuEntry> = MenuEntry::ALL
                .into_iter()
                .filter(|entry| entry.command() == Some(command))
                .collect();
            assert_eq!(entries.len(), 1, "{command:?}: {entries:?}");
        }
        let commands = MenuEntry::ALL
            .into_iter()
            .filter_map(MenuEntry::command)
            .count();
        assert_eq!(commands, CaptureCommand::ALL.len());
    }

    /// The list is the menu's: every entry once.
    #[test]
    fn every_entry_is_listed_once() {
        for (index, entry) in MenuEntry::ALL.into_iter().enumerate() {
            assert!(!MenuEntry::ALL[..index].contains(&entry), "{entry:?} twice");
        }
    }

    /// The title names the product and this agent's version, and comes first (RD-1260-08).
    #[test]
    fn the_title_comes_first_and_names_the_agents_version() {
        assert_eq!(MenuEntry::ALL[0], MenuEntry::Title);
        assert_eq!(
            title_line(),
            format!("rDownloader v{}", env!("CARGO_PKG_VERSION"))
        );
        assert!(!title_line().contains('&'), "no mnemonic in the title");
    }
}
