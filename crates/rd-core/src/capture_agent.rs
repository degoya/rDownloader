//! What the desktop capture agent is set to from the service: whether it watches the clipboard
//! (RD-1180-01), and the system-wide shortcuts of its tray commands (RD-1180-03).
//!
//! Shared by both ends on purpose. The service validates a shortcut before it stores one and the
//! agent turns the same text into a registration, so the two read one grammar: modifiers first,
//! one key last, joined by `+` — `CmdOrCtrl+Alt+V`. `CmdOrCtrl` is Ctrl on Windows and Linux and
//! Cmd on macOS, which is what lets one default serve every platform.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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

/// A shortcut, parsed: the modifiers and the one key.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Shortcut {
    /// Ctrl on Windows and Linux, Cmd on macOS.
    pub cmd_or_ctrl: bool,
    pub ctrl: bool,
    /// Alt; Option on macOS.
    pub alt: bool,
    pub shift: bool,
    /// The Windows key, Super on Linux, Cmd on macOS.
    pub super_key: bool,
    /// The key in its canonical spelling: `A`, `7`, `F5`, `Space`, `ArrowUp`.
    pub key: String,
}

/// The two ways `CmdOrCtrl` resolves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    /// Windows and Linux: `CmdOrCtrl` is Ctrl.
    Pc,
    /// macOS: `CmdOrCtrl` is Cmd.
    Mac,
}

impl Family {
    pub const BOTH: [Self; 2] = [Self::Pc, Self::Mac];
}

/// The keys a shortcut presses on one platform family, with `CmdOrCtrl` resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pressed {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    pub key: String,
}

/// Why a shortcut was not taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShortcutProblem {
    /// Not a shortcut this grammar knows: an unknown key or modifier, no key, two keys.
    Invalid,
    /// Fewer than two of Ctrl, Alt and Super/Cmd. With one, it is a program's own shortcut
    /// (copy, paste, a new tab) or a typed character (Option+letter on a Mac), and a system-wide
    /// registration would take that away everywhere.
    ModifierMissing,
    /// A combination the operating system or the desktop already uses.
    Reserved,
    /// Another command has it.
    Duplicate,
}

impl ShortcutProblem {
    /// The stable code the REST answer carries.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "capture.shortcut_invalid",
            Self::ModifierMissing => "capture.shortcut_modifier_missing",
            Self::Reserved => "capture.shortcut_reserved",
            Self::Duplicate => "capture.shortcut_duplicate",
        }
    }
}

/// A shortcut that was not taken, the command it was meant for and, for a duplicate, the
/// command that has it already.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShortcutRefusal {
    pub command: CaptureCommand,
    pub problem: ShortcutProblem,
    pub other: Option<CaptureCommand>,
}

/// Keys with a name rather than a character, in their canonical spelling.
const NAMED_KEYS: &[&str] = &[
    "Space",
    "Enter",
    "Tab",
    "Backspace",
    "Escape",
    "Insert",
    "Delete",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Backslash",
    "Semicolon",
    "Quote",
    "Comma",
    "Period",
    "Slash",
    "Backquote",
];

/// Combinations Windows or a Linux desktop already uses with two modifiers. Matched after
/// `CmdOrCtrl` is resolved, so `CmdOrCtrl+Alt+Delete` is caught as well.
const RESERVED_PC: &[&str] = &[
    "Ctrl+Alt+Delete",
    "Ctrl+Alt+Backspace",
    "Ctrl+Alt+Escape",
    "Ctrl+Alt+Tab",
    "Ctrl+Alt+T",
    "Ctrl+Alt+L",
    "Ctrl+Alt+D",
    "Ctrl+Alt+ArrowUp",
    "Ctrl+Alt+ArrowDown",
    "Ctrl+Alt+ArrowLeft",
    "Ctrl+Alt+ArrowRight",
    "Ctrl+Alt+Shift+ArrowUp",
    "Ctrl+Alt+Shift+ArrowDown",
    "Ctrl+Alt+Shift+ArrowLeft",
    "Ctrl+Alt+Shift+ArrowRight",
    "Ctrl+Super+D",
    "Ctrl+Super+F4",
    "Ctrl+Super+ArrowLeft",
    "Ctrl+Super+ArrowRight",
    "Ctrl+Super+Enter",
    "Ctrl+Super+O",
    "Ctrl+Super+C",
    "Ctrl+Super+S",
    "Ctrl+Super+N",
    "Ctrl+Super+Q",
    "Alt+Super+R",
    "Alt+Super+G",
    "Alt+Super+B",
    "Alt+Super+D",
    "Alt+Super+K",
    "Alt+Super+ArrowUp",
    "Alt+Super+ArrowDown",
];

/// The same for macOS, written with `Super` for Cmd and `Alt` for Option.
const RESERVED_MAC: &[&str] = &[
    "Super+Alt+Escape",
    "Super+Alt+D",
    "Super+Alt+H",
    "Super+Alt+M",
    "Super+Alt+W",
    "Super+Alt+Space",
    "Super+Alt+I",
    "Super+Alt+J",
    "Super+Alt+C",
    "Super+Alt+U",
    "Super+Alt+L",
    "Super+Alt+F",
    "Super+Alt+8",
    "Super+Alt+Minus",
    "Super+Alt+Equal",
    "Super+Ctrl+Q",
    "Super+Ctrl+F",
    "Super+Ctrl+Space",
    "Super+Ctrl+D",
];

impl Shortcut {
    /// Parses `Ctrl+Alt+K`, case-insensitive, with the usual aliases: `Control`, `Option`,
    /// `Cmd`/`Command`/`Win`/`Meta` for Super, `CommandOrControl`, `KeyK` and `Digit7`.
    ///
    /// Refuses what a system-wide registration must not take, so a shortcut that parses is one
    /// the agent may register.
    pub fn parse(text: &str) -> Result<Self, ShortcutProblem> {
        let shortcut = Self::read(text)?;
        let pressed = Family::BOTH.map(|family| shortcut.on(family));
        if pressed.iter().any(|keys| {
            [keys.ctrl, keys.alt, keys.super_key]
                .iter()
                .filter(|held| **held)
                .count()
                < 2
        }) {
            return Err(ShortcutProblem::ModifierMissing);
        }
        // All four at once is the Office key on Windows, which opens Office and its programs.
        if pressed
            .iter()
            .any(|keys| keys.ctrl && keys.alt && keys.shift && keys.super_key)
        {
            return Err(ShortcutProblem::Reserved);
        }
        for (family, reserved) in [(Family::Pc, RESERVED_PC), (Family::Mac, RESERVED_MAC)] {
            let keys = shortcut.on(family);
            if reserved
                .iter()
                .filter_map(|entry| Self::read(entry).ok())
                .any(|entry| entry.on(family) == keys)
            {
                return Err(ShortcutProblem::Reserved);
            }
        }
        Ok(shortcut)
    }

    /// The grammar alone, without the rules about which shortcuts may be registered.
    fn read(text: &str) -> Result<Self, ShortcutProblem> {
        let mut shortcut = Self {
            cmd_or_ctrl: false,
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
            key: String::new(),
        };
        let tokens: Vec<&str> = text.split('+').map(str::trim).collect();
        let Some((key, modifiers)) = tokens.split_last() else {
            return Err(ShortcutProblem::Invalid);
        };
        for modifier in modifiers {
            let held = match modifier.to_ascii_lowercase().as_str() {
                "cmdorctrl" | "commandorcontrol" | "cmdorcontrol" | "commandorctrl" => {
                    &mut shortcut.cmd_or_ctrl
                }
                "ctrl" | "control" => &mut shortcut.ctrl,
                "alt" | "option" => &mut shortcut.alt,
                "shift" => &mut shortcut.shift,
                "super" | "cmd" | "command" | "win" | "meta" => &mut shortcut.super_key,
                _ => return Err(ShortcutProblem::Invalid),
            };
            if *held {
                return Err(ShortcutProblem::Invalid);
            }
            *held = true;
        }
        // `CmdOrCtrl` together with what it stands for on one of the two would be one key named
        // twice on that platform.
        if shortcut.cmd_or_ctrl && (shortcut.ctrl || shortcut.super_key) {
            return Err(ShortcutProblem::Invalid);
        }
        shortcut.key = canonical_key(key).ok_or(ShortcutProblem::Invalid)?;
        Ok(shortcut)
    }

    /// The keys this presses on one platform family.
    #[must_use]
    pub fn on(&self, family: Family) -> Pressed {
        let (ctrl, super_key) = match family {
            Family::Pc => (self.ctrl || self.cmd_or_ctrl, self.super_key),
            Family::Mac => (self.ctrl, self.super_key || self.cmd_or_ctrl),
        };
        Pressed {
            ctrl,
            alt: self.alt,
            shift: self.shift,
            super_key,
            key: self.key.clone(),
        }
    }
}

/// The canonical spelling: `CmdOrCtrl`, `Ctrl`, `Alt`, `Shift`, `Super`, then the key. Both
/// `global-hotkey` and the tray's menu library read it as it stands.
impl fmt::Display for Shortcut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (held, name) in [
            (self.cmd_or_ctrl, "CmdOrCtrl"),
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.super_key, "Super"),
        ] {
            if held {
                write!(formatter, "{name}+")?;
            }
        }
        formatter.write_str(&self.key)
    }
}

/// A key token in its canonical spelling, or `None` for one this grammar does not know.
fn canonical_key(token: &str) -> Option<String> {
    let mut characters = token.chars();
    if let (Some(single), None) = (characters.next(), characters.next())
        && single.is_ascii_alphanumeric()
    {
        return Some(single.to_ascii_uppercase().to_string());
    }
    let lower = token.to_ascii_lowercase();
    for prefix in ["key", "digit"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let mut rest_characters = rest.chars();
            if let (Some(single), None) = (rest_characters.next(), rest_characters.next()) {
                let fits = if prefix == "key" {
                    single.is_ascii_alphabetic()
                } else {
                    single.is_ascii_digit()
                };
                if fits {
                    return Some(single.to_ascii_uppercase().to_string());
                }
            }
        }
    }
    if let Some(digits) = lower.strip_prefix('f')
        && !digits.starts_with('0')
        && let Ok(number) = digits.parse::<u8>()
        && (1..=24).contains(&number)
    {
        return Some(format!("F{number}"));
    }
    let alias = match lower.as_str() {
        "esc" => "escape",
        "up" => "arrowup",
        "down" => "arrowdown",
        "left" => "arrowleft",
        "right" => "arrowright",
        "return" => "enter",
        other => other,
    };
    NAMED_KEYS
        .iter()
        .find(|named| named.eq_ignore_ascii_case(alias))
        .map(|named| (*named).to_owned())
}

#[cfg(test)]
#[path = "capture_agent_tests.rs"]
mod tests;
