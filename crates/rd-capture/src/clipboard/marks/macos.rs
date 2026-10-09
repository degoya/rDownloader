//! The macOS marks: pasteboard types of the general pasteboard, asked for through the safe AppKit
//! bindings `arboard` reads the text with.

use anyhow::Result;
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSPasteboard;

use super::{MACOS_CONCEALED, MACOS_TRANSIENT, Mark};

pub(crate) struct Probe;

impl Probe {
    /// A probe that has not looked yet (one constructor on every platform: the unit struct of
    /// Windows and macOS is not built through `Default`).
    pub(crate) const fn new() -> Self {
        Self
    }

    /// The marks on the pasteboard now. Their content is never read: the type alone decides.
    pub(crate) fn marks(&mut self) -> Result<Vec<Mark>> {
        // A pool of its own: this runs on the blocking pool, where nothing drains one.
        let offered: Vec<String> = autoreleasepool(|_| {
            NSPasteboard::generalPasteboard()
                .types()
                .map(|types| types.to_vec().iter().map(|name| name.to_string()).collect())
                .unwrap_or_default()
        });
        Ok([MACOS_CONCEALED, MACOS_TRANSIENT]
            .into_iter()
            .filter(|format| offered.iter().any(|offered| offered == format))
            .map(|format| Mark {
                format,
                value: Some(Vec::new()),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use arboard::{Clipboard, SetExtApple};

    use super::{super::concealed, Probe};

    /// Written the way a password manager writes it — through `arboard`, which sets
    /// `org.nspasteboard.ConcealedType` — and read back by the probe from the real pasteboard.
    #[test]
    fn a_concealed_copy_reads_as_concealed() {
        let mut clipboard = Clipboard::new().expect("open the pasteboard");
        clipboard
            .set()
            .exclude_from_history()
            .text("https://files.example.com/one-time?key=abc")
            .expect("write a concealed copy");
        let marks = Probe.marks().expect("ask for the marks");
        assert!(concealed(&marks), "{marks:?}");
        clipboard
            .set_text("https://files.example.com/plain")
            .expect("write a plain copy");
        let marks = Probe.marks().expect("ask for the marks");
        assert!(!concealed(&marks), "{marks:?}");
    }
}
