//! The shortcut grammar both ends of the capture agent read: modifiers first, one key last,
//! joined by `+` -- `CmdOrCtrl+Alt+V`. Parsing, the canonical spelling, the keys a shortcut
//! presses on each platform family, and the combinations the systems keep for themselves.

use std::fmt;

use super::CaptureCommand;

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
pub(super) const RESERVED_PC: &[&str] = &[
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
pub(super) const RESERVED_MAC: &[&str] = &[
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
    pub(crate) fn read(text: &str) -> Result<Self, ShortcutProblem> {
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
