//! The rule over the marks, one case per platform's convention. The probes behind it ask the
//! real clipboard and are compiled per platform; what they report is held here.

use super::{
    KDE_HINT, MACOS_CONCEALED, MACOS_TRANSIENT, Mark, WINDOWS_EXCLUDE, WINDOWS_HISTORY, concealed,
};

fn mark(format: &'static str, value: &[u8]) -> Mark {
    Mark {
        format,
        value: Some(value.to_vec()),
    }
}

fn unreadable(format: &'static str) -> Mark {
    Mark {
        format,
        value: None,
    }
}

#[test]
fn a_clipboard_without_marks_is_taken() {
    assert!(!concealed(&[]));
}

#[test]
fn windows_exclusion_from_monitoring_conceals_whatever_it_holds() {
    assert!(concealed(&[mark(WINDOWS_EXCLUDE, b"")]));
    assert!(concealed(&[mark(WINDOWS_EXCLUDE, &1_u32.to_le_bytes())]));
    assert!(concealed(&[unreadable(WINDOWS_EXCLUDE)]));
}

#[test]
fn windows_history_hint_conceals_only_when_it_forbids_the_history() {
    assert!(concealed(&[mark(WINDOWS_HISTORY, &0_u32.to_le_bytes())]));
    assert!(!concealed(&[mark(WINDOWS_HISTORY, &1_u32.to_le_bytes())]));
    // Too short to be a DWORD, or not readable at all: taken as forbidding it.
    assert!(concealed(&[mark(WINDOWS_HISTORY, &[1, 0])]));
    assert!(concealed(&[unreadable(WINDOWS_HISTORY)]));
}

#[test]
fn macos_concealed_and_transient_types_conceal_by_their_presence() {
    assert!(concealed(&[mark(MACOS_CONCEALED, b"")]));
    assert!(concealed(&[mark(MACOS_TRANSIENT, b"")]));
}

#[test]
fn the_kde_hint_conceals_when_it_says_secret() {
    assert!(concealed(&[mark(KDE_HINT, b"secret")]));
    // As password managers and C strings write it.
    assert!(concealed(&[mark(KDE_HINT, b"secret\0")]));
    assert!(concealed(&[mark(KDE_HINT, b" Secret\n")]));
    // Offered, but its content could not be read: the safe reading.
    assert!(concealed(&[unreadable(KDE_HINT)]));
    assert!(!concealed(&[mark(KDE_HINT, b"public")]));
}

#[test]
fn one_concealing_mark_among_others_is_enough() {
    assert!(concealed(&[
        mark(WINDOWS_HISTORY, &1_u32.to_le_bytes()),
        mark(WINDOWS_EXCLUDE, b""),
    ]));
    assert!(!concealed(&[
        mark(WINDOWS_HISTORY, &1_u32.to_le_bytes()),
        mark(KDE_HINT, b"public"),
    ]));
}
