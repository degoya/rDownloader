//! Every entry of the tray menu and the command each one carries out (RD-1240-24).
//!
//! The tray's handle builds its items for these entries and turns a click into the entry's
//! command, which `controls::action` carries out the same way as the command's shortcut. Every
//! entry a click can choose has a command, so every tray function can be put on a shortcut
//! (owner, 2026-10-10; none is assigned by default). The tests below hold that for every entry
//! to come: the tray is compiled on Windows and macOS only, this list on every host.

use rd_core::CaptureCommand;

/// One entry of the tray menu, separators aside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuEntry {
    /// The agent's line at the top (RD-1100-06).
    StatusLine,
    /// The server's line under it (RD-1240-06).
    ServerLine,
    Open,
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
    pub(crate) const ALL: [Self; 18] = [
        Self::StatusLine,
        Self::ServerLine,
        Self::Open,
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
        Self::ServerUpdate,
        Self::RestartServer,
        Self::Update,
        Self::AutoInstall,
        Self::Quit,
    ];

    /// Whether a click can ever choose the entry. The two lines and the hint are only read: the
    /// tray builds them greyed out and never enables them. Every other entry can be chosen at
    /// least some of the time, and is a function a shortcut can reach.
    pub(crate) fn clickable(self) -> bool {
        match self {
            Self::StatusLine | Self::ServerLine | Self::PairHint => false,
            Self::Open
            | Self::StartAll
            | Self::PauseAll
            | Self::PauseHalfHour
            | Self::PauseHour
            | Self::AddAll
            | Self::AddAllPaused
            | Self::ClipboardWatch
            | Self::SendClipboard
            | Self::GameMode
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
            Self::StatusLine | Self::ServerLine | Self::PairHint => return None,
            Self::Open => CaptureCommand::Open,
            Self::StartAll => CaptureCommand::StartAll,
            Self::PauseAll => CaptureCommand::PauseAll,
            Self::PauseHalfHour => CaptureCommand::PauseHalfHour,
            Self::PauseHour => CaptureCommand::PauseHour,
            Self::AddAll => CaptureCommand::AddAllFromLinkGrabber,
            Self::AddAllPaused => CaptureCommand::AddAllFromLinkGrabberPaused,
            Self::ClipboardWatch => CaptureCommand::ClipboardWatch,
            Self::SendClipboard => CaptureCommand::SendClipboard,
            Self::GameMode => CaptureCommand::GameMode,
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

    use super::MenuEntry;

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
}
