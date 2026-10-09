//! The marks with which a password manager says "do not take this" (RD-1200-03,
//! `docs/security/capture-agent.md` finding 3).
//!
//! The clipboard crate reads text only, so each platform asks for its own marks beside it: the
//! registered formats `ExcludeClipboardContentFromMonitorProcessing` and
//! `CanIncludeInClipboardHistory` on Windows, the pasteboard types `org.nspasteboard.ConcealedType`
//! and `org.nspasteboard.TransientType` on macOS (nspasteboard.org), and the target or MIME type
//! `x-kde-passwordManagerHint` with the content `secret` under X11 and Wayland. Every probe reports
//! which of these marks the clipboard offers, with the content where the rule reads one, and one
//! decision over all of them — [`concealed`] — is what the tests hold.

/// Windows: a clipboard viewer or monitor leaves this content alone, whatever the format holds.
pub(crate) const WINDOWS_EXCLUDE: &str = "ExcludeClipboardContentFromMonitorProcessing";
/// Windows: a DWORD; `0` keeps the content out of the clipboard history. A hint, taken as one.
pub(crate) const WINDOWS_HISTORY: &str = "CanIncludeInClipboardHistory";
/// macOS: the content is a password or other secret.
pub(crate) const MACOS_CONCEALED: &str = "org.nspasteboard.ConcealedType";
/// macOS: the content is gone again shortly and is not to be kept.
pub(crate) const MACOS_TRANSIENT: &str = "org.nspasteboard.TransientType";
/// X11 and Wayland (KDE's convention, which KeePassXC and others follow): `secret` marks it.
pub(crate) const KDE_HINT: &str = "x-kde-passwordManagerHint";

/// One mark the clipboard offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mark {
    /// One of the names above.
    pub(crate) format: &'static str,
    /// What the mark holds; `None` when it is offered but could not be read. Empty for a mark
    /// whose presence alone decides.
    pub(crate) value: Option<Vec<u8>>,
}

/// Whether the clipboard content behind `marks` is to be left alone.
///
/// A mark that is offered but whose content could not be read counts as saying so: the cost of
/// that mistake is one copy not handed over, the cost of the other is a password at the service.
pub(crate) fn concealed(marks: &[Mark]) -> bool {
    marks.iter().any(|mark| match mark.format {
        WINDOWS_EXCLUDE | MACOS_CONCEALED | MACOS_TRANSIENT => true,
        WINDOWS_HISTORY => mark.value.as_deref().is_none_or(|value| {
            value
                .first_chunk::<4>()
                .is_none_or(|dword| u32::from_le_bytes(*dword) == 0)
        }),
        KDE_HINT => mark.value.as_deref().is_none_or(|value| {
            value
                .strip_suffix(b"\0")
                .unwrap_or(value)
                .trim_ascii()
                .eq_ignore_ascii_case(b"secret")
        }),
        _ => false,
    })
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(all(unix, not(target_os = "macos")))]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(all(unix, not(target_os = "macos")))]
mod x11;

#[cfg(target_os = "macos")]
pub(crate) use macos::Probe;
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) use unix::Probe;
#[cfg(windows)]
pub(crate) use windows::Probe;

#[cfg(test)]
mod tests;
