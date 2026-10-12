//! Files handed to the agent from the desktop (RD-1260-09): opened with it, dropped on its desktop
//! shortcut or sent to it with "Send to" -- all of them end in `rdownloader-capture open`.
//!
//! Which file goes where, the one list of extensions the agent registers, and the one notice a
//! hand-over of several files ends with are decided here, on every host; `commands::open` only
//! reads the files and sends them.

use std::path::Path;

/// What a handed-over file is, read from its extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileKind {
    /// An NZB: `capture/nzb`, as before.
    Nzb,
    /// A link container: `capture/file`, which takes it by its name through the LinkGrabber's
    /// container import.
    Container,
    /// A torrent: `capture/file` too, which takes it by its content as it takes the extension's.
    Torrent,
}

/// Every extension the agent registers with the desktop and takes in `open`, lower case and with
/// its dot, as the registry and the desktop entries spell one.
///
/// `.txt` is a container format of the service too, but far too general to claim. `.metalink`
/// and `.meta4` are left out: the service reads them only as pasted text, through the Metalink
/// intake parser, and has no route that takes one as a file.
pub(crate) const HANDED_OVER: [(&str, FileKind); 7] = [
    (".nzb", FileKind::Nzb),
    (".dlc", FileKind::Container),
    (".ccf", FileKind::Container),
    (".rsdf", FileKind::Container),
    (".rdlinks", FileKind::Container),
    (".crawljob", FileKind::Container),
    (".torrent", FileKind::Torrent),
];

/// The stable code a file of another type is refused with, here and in the notice.
pub(crate) const UNSUPPORTED: &str = "capture.file_type_unsupported";

/// A file `open` does not take, with the extension it named.
#[derive(Debug)]
pub(crate) struct Unsupported {
    pub(crate) extension: String,
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.extension.is_empty() {
            write!(formatter, "{UNSUPPORTED}: a file without an extension")
        } else {
            write!(
                formatter,
                "{UNSUPPORTED}: .{} is not a type rDownloader takes",
                self.extension
            )
        }
    }
}

impl std::error::Error for Unsupported {}

/// The kind of `path`, by its extension whatever its case.
pub(crate) fn kind_of(path: &Path) -> Result<FileKind, Unsupported> {
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    HANDED_OVER
        .iter()
        .find(|(known, _)| known.strip_prefix('.') == Some(extension.as_str()))
        .map(|(_, kind)| *kind)
        .ok_or(Unsupported { extension })
}

/// What one `open` came to, file by file.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// The names of the files the service took.
    pub(crate) handed: Vec<String>,
    /// The names of the files it did not take, each with the code or the reason.
    pub(crate) refused: Vec<(String, String)>,
}

impl Outcome {
    /// Whether the desktop is told: always for several files and for every refusal. A single
    /// file that arrived is announced by the LinkGrabber's own notification.
    pub(crate) fn tells(&self) -> bool {
        !self.refused.is_empty() || self.handed.len() > 1
    }

    /// The one line for all of them: how many arrived, and which did not, by name and code.
    pub(crate) fn summary(&self) -> String {
        let total = self.handed.len() + self.refused.len();
        let handed = match (self.handed.len(), total) {
            (1, 1) => format!("Handed to rDownloader: {}", self.handed[0]),
            (count, total) if count == total => format!("{count} files handed to rDownloader"),
            (count, total) => format!("{count} of {total} files handed to rDownloader"),
        };
        if self.refused.is_empty() {
            return handed;
        }
        let refused: Vec<String> = self
            .refused
            .iter()
            .map(|(name, reason)| format!("{name} ({reason})"))
            .collect();
        format!("{handed}; not taken: {}", refused.join(", "))
    }
}

/// The name a file is reported under: its own name, not the folder it lies in.
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
#[path = "hand_over_tests.rs"]
mod tests;
