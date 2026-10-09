//! The Linux and BSD mark: `x-kde-passwordManagerHint`, asked for over Wayland's data-control
//! protocol where a compositor offers it and over X11 otherwise — the same choice `arboard`
//! makes for the text, with the same protocol clients it links.

use std::io::Read as _;

use anyhow::Result;
use wl_clipboard_rs::paste::{
    ClipboardType, Error as PasteError, MimeType, Seat, get_contents, get_mime_types,
};

use super::{KDE_HINT, Mark, x11::Selection};

/// Longest mark content read; `secret` is six bytes.
const MAX_MARK_BYTES: u64 = 64;

#[derive(Default)]
pub(crate) struct Probe {
    /// The X11 connection, kept between looks and opened again after a failure.
    x11: Option<Selection>,
}

impl Probe {
    /// A probe that has not looked yet (one constructor on every platform: the unit struct of
    /// Windows and macOS is not built through `Default`).
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn marks(&mut self) -> Result<Vec<Mark>> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            match wayland_marks() {
                Ok(marks) => return Ok(marks),
                // No data-control protocol, or no compositor after all: X11, as arboard does.
                Err(error) => {
                    tracing::debug!(%error, "Wayland clipboard marks unavailable; asking X11")
                }
            }
        }
        let selection = match self.x11.take() {
            Some(selection) => selection,
            None => Selection::open()?,
        };
        let marks = selection.marks()?;
        self.x11 = Some(selection);
        Ok(marks)
    }
}

fn wayland_marks() -> Result<Vec<Mark>> {
    let offered = match get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
        Ok(offered) => offered,
        // An empty clipboard carries no mark.
        Err(PasteError::NoSeats | PasteError::ClipboardEmpty | PasteError::NoMimeType) => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(anyhow::anyhow!("the Wayland clipboard: {error}")),
    };
    if !offered.contains(KDE_HINT) {
        return Ok(Vec::new());
    }
    let value = get_contents(
        ClipboardType::Regular,
        Seat::Unspecified,
        MimeType::Specific(KDE_HINT),
    )
    .ok()
    .and_then(|(pipe, _)| {
        let mut value = Vec::new();
        pipe.take(MAX_MARK_BYTES)
            .read_to_end(&mut value)
            .ok()
            .map(|_| value)
    });
    Ok(vec![Mark {
        format: KDE_HINT,
        value,
    }])
}
