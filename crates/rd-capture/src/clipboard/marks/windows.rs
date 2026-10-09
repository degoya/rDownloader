//! The Windows marks: two registered clipboard formats, asked for through `clipboard-win`'s safe
//! calls — the crate `arboard` reads the text with.

use anyhow::Result;
use clipboard_win::{Clipboard, formats::RawData, get, is_format_avail, register_format, size};

use super::{Mark, WINDOWS_EXCLUDE, WINDOWS_HISTORY};

/// How often opening the clipboard is tried while another program holds it.
const OPEN_ATTEMPTS: usize = 10;

/// Longest mark content read; `CanIncludeInClipboardHistory` is one DWORD.
const MAX_MARK_BYTES: usize = 64;

pub(crate) struct Probe;

impl Probe {
    /// A probe that has not looked yet (one constructor on every platform: the unit struct of
    /// Windows and macOS is not built through `Default`).
    pub(crate) const fn new() -> Self {
        Self
    }

    /// The marks on the clipboard now. The clipboard is open for the length of this call only.
    pub(crate) fn marks(&mut self) -> Result<Vec<Mark>> {
        let _open = Clipboard::new_attempts(OPEN_ATTEMPTS)
            .map_err(|error| anyhow::anyhow!("open the clipboard: {error}"))?;
        let mut marks = Vec::new();
        if offered(WINDOWS_EXCLUDE).is_some() {
            marks.push(Mark {
                format: WINDOWS_EXCLUDE,
                value: Some(Vec::new()),
            });
        }
        if let Some(format) = offered(WINDOWS_HISTORY) {
            let value = size(format)
                .is_some_and(|length| length.get() <= MAX_MARK_BYTES)
                .then(|| get::<Vec<u8>, _>(RawData(format)).ok())
                .flatten();
            marks.push(Mark {
                format: WINDOWS_HISTORY,
                value,
            });
        }
        Ok(marks)
    }
}

/// The format registered under `name`, when the clipboard holds it now.
fn offered(name: &str) -> Option<u32> {
    let format = register_format(name)?.get();
    is_format_avail(format).then_some(format)
}

#[cfg(test)]
mod tests {
    use arboard::{Clipboard, SetExtWindows};

    use super::{super::concealed, Probe};

    /// Written the way a password manager writes it — through `arboard`, which sets the same
    /// formats — and read back by the probe from the real clipboard.
    #[test]
    fn a_copy_kept_from_monitoring_or_history_reads_as_concealed() {
        let mut clipboard = Clipboard::new().expect("open the clipboard");
        let link = "https://files.example.com/one-time?key=abc";
        clipboard
            .set()
            .exclude_from_monitoring()
            .text(link)
            .expect("write a marked copy");
        let marks = Probe.marks().expect("ask for the marks");
        assert!(concealed(&marks), "{marks:?}");
        clipboard
            .set()
            .exclude_from_history()
            .text(link)
            .expect("write a copy kept from the history");
        let marks = Probe.marks().expect("ask for the marks");
        assert!(concealed(&marks), "{marks:?}");
        clipboard
            .set_text("https://files.example.com/plain")
            .expect("write a plain copy");
        let marks = Probe.marks().expect("ask for the marks");
        assert!(!concealed(&marks), "{marks:?}");
    }
}
