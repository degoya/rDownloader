//! The shape of a system check, shared by `rdownloader doctor` and the diagnostic bundle.
//!
//! The checks themselves are collected in `rd-api` (`diagnostics_checks.rs`), which links the
//! tool and proxy crates they read; what lives here is the record and the text rendering the
//! `doctor` command prints, so the bundle's `doctor.txt` is the command's output and not a
//! second reading of the same facts.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What a check found.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// Present and usable.
    Ok,
    /// Present, with something a person should read.
    Warning,
    /// Not found, or not usable.
    Missing,
    /// A fact, not a verdict: a path, an address.
    Info,
}

/// One line of `doctor` output, with its verdict kept separate from its words.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct Check {
    /// The heading the line is printed under, such as `download helpers`.
    pub section: String,
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    /// Indented lines under the check: a version, an upgrade path, a warning.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Check {
    #[must_use]
    pub fn new(
        section: impl Into<String>,
        name: impl Into<String>,
        status: CheckStatus,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            section: section.into(),
            name: name.into(),
            status,
            detail: detail.into(),
            notes: Vec::new(),
        }
    }

    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// The text `rdownloader doctor` prints for these checks: a heading per section in first
/// appearance order, two spaces per check, six per note, and `!!` in front of a warning.
#[must_use]
pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    let mut current: Option<&str> = None;
    for check in checks {
        if current != Some(check.section.as_str()) {
            out.push_str(&check.section);
            out.push_str(":\n");
            current = Some(check.section.as_str());
        }
        let marker = match check.status {
            CheckStatus::Warning | CheckStatus::Missing => "!! ",
            CheckStatus::Ok | CheckStatus::Info => "",
        };
        out.push_str(&format!("  {marker}{}: {}\n", check.name, check.detail));
        for note in &check.notes {
            out.push_str(&format!("      {note}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Check, CheckStatus, render};

    #[test]
    fn rendering_groups_by_section_and_marks_what_needs_reading() {
        let checks = vec![
            Check::new(
                "download helpers",
                "yt-dlp",
                CheckStatus::Ok,
                "/usr/bin/yt-dlp (Path)",
            )
            .note("version: 2026.01.01 - compatible"),
            Check::new(
                "download helpers",
                "unrar",
                CheckStatus::Missing,
                "not found",
            ),
            Check::new(
                "Reverse proxy",
                "external URL",
                CheckStatus::Info,
                "not set",
            ),
            Check::new(
                "Reverse proxy",
                "cookie",
                CheckStatus::Warning,
                "not Secure over https",
            ),
        ];
        assert_eq!(
            render(&checks),
            "download helpers:\n  yt-dlp: /usr/bin/yt-dlp (Path)\n      version: 2026.01.01 - compatible\n  !! unrar: not found\nReverse proxy:\n  external URL: not set\n  !! cookie: not Secure over https\n"
        );
    }
}
