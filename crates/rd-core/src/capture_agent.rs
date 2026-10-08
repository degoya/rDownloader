//! What the desktop capture agent is set to from the service: whether it watches the clipboard
//! (RD-1180-01), and the system-wide shortcuts of its tray commands (RD-1180-03).
//!
//! Shared by both ends on purpose. The service validates a shortcut before it stores one and the
//! agent turns the same text into a registration, so the two read one grammar: modifiers first,
//! one key last, joined by `+` — `CmdOrCtrl+Alt+V`. `CmdOrCtrl` is Ctrl on Windows and Linux and
//! Cmd on macOS, which is what lets one default serve every platform.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[path = "capture_shortcut.rs"]
mod shortcut;

pub use shortcut::{Family, Pressed, Shortcut, ShortcutProblem, ShortcutRefusal};
#[cfg(test)]
use shortcut::{RESERVED_MAC, RESERVED_PC};

/// One command of the agent's tray menu, each of which a shortcut can trigger.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CaptureCommand {
    /// "Open rDownloader".
    Open,
    /// "Start all".
    StartAll,
    /// "Pause all".
    PauseAll,
    /// "Pause for 30 minutes".
    PauseHalfHour,
    /// "Pause for 1 hour".
    PauseHour,
    /// "Pause clipboard watching", switched on and off (RD-1180-01).
    ClipboardWatch,
    /// "Hand over clipboard now": the clipboard read once, watching paused or not.
    SendClipboard,
    /// "Quit".
    Quit,
}

impl CaptureCommand {
    /// Every command, in the order of the tray menu.
    pub const ALL: [Self; 8] = [
        Self::Open,
        Self::StartAll,
        Self::PauseAll,
        Self::PauseHalfHour,
        Self::PauseHour,
        Self::ClipboardWatch,
        Self::SendClipboard,
        Self::Quit,
    ];

    /// The stable name, as the settings document and the API spell it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::StartAll => "start_all",
            Self::PauseAll => "pause_all",
            Self::PauseHalfHour => "pause_half_hour",
            Self::PauseHour => "pause_hour",
            Self::ClipboardWatch => "clipboard_watch",
            Self::SendClipboard => "send_clipboard",
            Self::Quit => "quit",
        }
    }
}

/// The shortcut of every command; `None` is "no shortcut".
///
/// A field left out of a stored or sent document takes its default, while an explicit `null` is
/// "no shortcut": somebody who removed one must not get it back from the next release.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CaptureShortcuts {
    #[serde(default = "default_open")]
    pub open: Option<String>,
    #[serde(default = "default_start_all")]
    pub start_all: Option<String>,
    #[serde(default = "default_pause_all")]
    pub pause_all: Option<String>,
    #[serde(default = "default_pause_half_hour")]
    pub pause_half_hour: Option<String>,
    #[serde(default = "default_pause_hour")]
    pub pause_hour: Option<String>,
    #[serde(default = "default_clipboard_watch")]
    pub clipboard_watch: Option<String>,
    #[serde(default = "default_send_clipboard")]
    pub send_clipboard: Option<String>,
    /// No default: quitting by accident is the one command a stray key press should not reach.
    #[serde(default)]
    pub quit: Option<String>,
}

// The defaults (owner, 2026-10-07: Ctrl+Alt+<letter> on Windows and Linux, Cmd+Option+<letter> on
// macOS). The letters avoid what the systems and the common programs already take with these two
// modifiers, and the AltGr letters of the German, French and Spanish layouts (Q, E, M and the
// digits), because Windows reads AltGr as Ctrl+Alt. The table and its reasons are in RD-1180-03.
fn default_open() -> Option<String> {
    Some("CmdOrCtrl+Alt+O".to_owned())
}
fn default_start_all() -> Option<String> {
    Some("CmdOrCtrl+Alt+G".to_owned())
}
fn default_pause_all() -> Option<String> {
    Some("CmdOrCtrl+Alt+P".to_owned())
}
fn default_pause_half_hour() -> Option<String> {
    Some("CmdOrCtrl+Alt+K".to_owned())
}
fn default_pause_hour() -> Option<String> {
    Some("CmdOrCtrl+Alt+Shift+K".to_owned())
}
fn default_clipboard_watch() -> Option<String> {
    Some("CmdOrCtrl+Alt+Z".to_owned())
}
fn default_send_clipboard() -> Option<String> {
    Some("CmdOrCtrl+Alt+V".to_owned())
}

impl Default for CaptureShortcuts {
    fn default() -> Self {
        Self {
            open: default_open(),
            start_all: default_start_all(),
            pause_all: default_pause_all(),
            pause_half_hour: default_pause_half_hour(),
            pause_hour: default_pause_hour(),
            clipboard_watch: default_clipboard_watch(),
            send_clipboard: default_send_clipboard(),
            quit: None,
        }
    }
}

impl CaptureShortcuts {
    /// The shortcut of one command.
    #[must_use]
    pub fn get(&self, command: CaptureCommand) -> Option<&str> {
        self.slot(command).as_deref()
    }

    /// Sets or clears the shortcut of one command.
    pub fn set(&mut self, command: CaptureCommand, shortcut: Option<String>) {
        *self.slot_mut(command) = shortcut;
    }

    fn slot(&self, command: CaptureCommand) -> &Option<String> {
        match command {
            CaptureCommand::Open => &self.open,
            CaptureCommand::StartAll => &self.start_all,
            CaptureCommand::PauseAll => &self.pause_all,
            CaptureCommand::PauseHalfHour => &self.pause_half_hour,
            CaptureCommand::PauseHour => &self.pause_hour,
            CaptureCommand::ClipboardWatch => &self.clipboard_watch,
            CaptureCommand::SendClipboard => &self.send_clipboard,
            CaptureCommand::Quit => &self.quit,
        }
    }

    fn slot_mut(&mut self, command: CaptureCommand) -> &mut Option<String> {
        match command {
            CaptureCommand::Open => &mut self.open,
            CaptureCommand::StartAll => &mut self.start_all,
            CaptureCommand::PauseAll => &mut self.pause_all,
            CaptureCommand::PauseHalfHour => &mut self.pause_half_hour,
            CaptureCommand::PauseHour => &mut self.pause_hour,
            CaptureCommand::ClipboardWatch => &mut self.clipboard_watch,
            CaptureCommand::SendClipboard => &mut self.send_clipboard,
            CaptureCommand::Quit => &mut self.quit,
        }
    }

    /// Every shortcut in its canonical spelling, or the first one that is refused.
    ///
    /// Checked one by one first, then against each other on both platform families: two
    /// spellings that press the same keys on one of them — `CmdOrCtrl+Alt+K` and `Ctrl+Alt+K` on
    /// Windows — are the same shortcut there.
    pub fn validated(&self) -> Result<Self, ShortcutRefusal> {
        let mut normalized = self.clone();
        let mut parsed: Vec<(CaptureCommand, Shortcut)> = Vec::new();
        for command in CaptureCommand::ALL {
            let Some(text) = self.get(command) else {
                continue;
            };
            let shortcut = Shortcut::parse(text).map_err(|problem| ShortcutRefusal {
                command,
                problem,
                other: None,
            })?;
            normalized.set(command, Some(shortcut.to_string()));
            parsed.push((command, shortcut));
        }
        for (index, (command, shortcut)) in parsed.iter().enumerate() {
            for (other, earlier) in &parsed[..index] {
                if Family::BOTH
                    .iter()
                    .any(|family| shortcut.on(*family) == earlier.on(*family))
                {
                    return Err(ShortcutRefusal {
                        command: *command,
                        problem: ShortcutProblem::Duplicate,
                        other: Some(*other),
                    });
                }
            }
        }
        Ok(normalized)
    }
}

/// What the service holds for the agent.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CaptureAgentSettings {
    /// Whether the agent leaves the clipboard alone (RD-1180-01). Click'n'Load, the browser
    /// extension and the `rdownloader://` scheme stay on: they are something somebody does, the
    /// clipboard is something that happens. What is copied while this holds is never delivered.
    #[serde(default)]
    pub clipboard_paused: bool,
    #[serde(default)]
    pub shortcuts: CaptureShortcuts,
}

/// Why the agent cannot register any shortcut at all.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutsUnavailable {
    /// A Wayland session: it has no global shortcuts a program may claim.
    Wayland,
    /// No display server to register them with (Linux without `DISPLAY`).
    NoDisplay,
    /// The agent runs without its tray (`--no-tray`), and the tray's event loop is what
    /// receives them on Windows and macOS.
    NoTray,
    /// The system refused to set the registration up.
    Failed,
}

/// The operating system an agent runs on, so a shortcut can be shown with its own key names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapturePlatform {
    Windows,
    Macos,
    Linux,
}

/// What an agent last said about registering its shortcuts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
pub struct CaptureShortcutReport {
    pub platform: CapturePlatform,
    /// Commands whose shortcut the system refused: another program holds the combination.
    #[serde(default)]
    pub refused: Vec<CaptureCommand>,
    /// Set when no shortcut could be registered at all.
    #[serde(default)]
    pub unavailable: Option<ShortcutsUnavailable>,
    /// When the service received it; whatever the agent sends here is replaced.
    #[serde(default)]
    pub reported_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
#[path = "capture_agent_tests.rs"]
mod tests;
