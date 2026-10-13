//! `[[extension.settings]]`: what a notification destination may be set to (RD-170-09) -- a
//! choice among the values its manifest lists, or free text (RD-1250-01).
//!
//! Free text is held to the host's rule, not the plugin's: at most [`MAX_SETTING_LINE_CHARS`]
//! characters on one line, or [`MAX_SETTING_TEXT_CHARS`] where the manifest says `multiline`,
//! and no control character other than the line breaks of a multi-line one. The same rule
//! refuses a value when the target is saved and drops a stored one at delivery, so a plugin
//! never reads text it was not promised.

use anyhow::{Result, bail};

use super::checks::bounded_text;
use super::{MAX_SLUG_CHARS, SettingManifest};

/// Most settings one destination declares, and most values one setting offers.
const MAX_SETTINGS: usize = 16;
const MAX_SETTING_CHOICES: usize = 32;

/// Most characters a one-line free-text setting holds: a library section, a name.
pub const MAX_SETTING_LINE_CHARS: usize = 256;

/// Most characters a multi-line free-text setting holds: a mapping of a few dozen folders.
pub const MAX_SETTING_TEXT_CHARS: usize = 4096;

impl SettingManifest {
    /// Whether the setting is free text rather than a choice: it lists no values.
    #[must_use]
    pub(crate) fn is_free_text(&self) -> bool {
        self.choices.is_empty()
    }

    /// Whether `value` is one this setting accepts: one of its choices, or free text within
    /// the host's rule. The empty value is the caller's to read -- "left alone" when a target
    /// is saved.
    #[must_use]
    pub fn accepts(&self, value: &str) -> bool {
        if !self.is_free_text() {
            return self.choices.iter().any(|choice| choice == value);
        }
        let limit = if self.multiline {
            MAX_SETTING_TEXT_CHARS
        } else {
            MAX_SETTING_LINE_CHARS
        };
        value.chars().count() <= limit
            && value.chars().all(|character| {
                !character.is_control() || (self.multiline && is_line_break(character))
            })
    }
}

/// The line breaks a multi-line setting may hold, in either spelling a browser sends.
fn is_line_break(character: char) -> bool {
    character == '\n' || character == '\r'
}

/// `[[extension.settings]]`: names that can be a key and a code, values that can be offered.
pub(super) fn validate_settings(settings: &[SettingManifest]) -> Result<()> {
    if settings.len() > MAX_SETTINGS {
        bail!("extension.settings declares more than {MAX_SETTINGS} settings");
    }
    let mut names = Vec::new();
    for setting in settings {
        let name = setting.name.as_str();
        if name.is_empty()
            || name.len() > MAX_SLUG_CHARS
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            bail!("extension.settings name `{name}` must be 1-{MAX_SLUG_CHARS} of a-z, 0-9 and _");
        }
        if names.contains(&name) {
            bail!("extension.settings declares `{name}` twice");
        }
        names.push(name);
        if setting.is_free_text() {
            validate_free_text(setting)?;
            continue;
        }
        if setting.multiline {
            bail!("extension.settings `{name}` offers choices, so it cannot be multiline");
        }
        if setting.choices.len() > MAX_SETTING_CHOICES {
            bail!("extension.settings `{name}` needs 1-{MAX_SETTING_CHOICES} choices");
        }
        for (index, choice) in setting.choices.iter().enumerate() {
            bounded_text("extension.settings choice", choice, MAX_SLUG_CHARS)?;
            if choice.trim() != choice || setting.choices[..index].contains(choice) {
                bail!("extension.settings `{name}` offers `{choice}` badly or twice");
            }
        }
        if let Some(default) = &setting.default
            && !setting.choices.contains(default)
        {
            bail!("extension.settings `{name}` defaults to `{default}`, which it does not offer");
        }
    }
    Ok(())
}

/// A free-text setting's default is text the person could have entered themselves.
fn validate_free_text(setting: &SettingManifest) -> Result<()> {
    if let Some(default) = &setting.default
        && (default.is_empty() || !setting.accepts(default))
    {
        bail!(
            "extension.settings `{}` defaults to text the setting would refuse",
            setting.name
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
