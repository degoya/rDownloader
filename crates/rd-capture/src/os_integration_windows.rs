//! The `HKCU\Software\Classes` entries `association` and `scheme` write on Windows, and the
//! keys their removal deletes: one list for both sides (RD-109-15). Pure and cfg-free, so the
//! exact keys, values and quoting are tested on every host; only `os_integration`'s Windows
//! functions write them.

use super::{Kind, WINDOWS_APP_ID};
use crate::hand_over::{FileKind, HANDED_OVER};

/// The file name the agent is registered under in `Applications` (RD-1260-09): the binary's own.
const EXECUTABLE_NAME: &str = "rdownloader-capture.exe";

/// One `HKCU\Software\Classes` entry the capture agent writes, and how far removing it may go.
///
/// Installing and removing used to be two independent enumerations -- a list of tuples on one
/// side, a sequence of calls on the other -- with nothing connecting them, so a key added to one
/// side and not the other went unnoticed. That is exactly what happened to the
/// `AppUserModelId` entry, which `association install` wrote and `association remove` left
/// behind for good (RD-109-15).
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct RegistryEntry {
    pub(super) key: String,
    /// The value name, or `None` for the key's default value.
    pub(super) name: Option<&'static str>,
    pub(super) value: String,
    /// The key `remove` deletes for this entry, with everything under it.
    ///
    /// Usually the top of the entry's own branch, so several entries under one branch collapse
    /// to a single delete. For the `SystemFileAssociations\.nzb` entries it is deliberately
    /// *not* the top of the branch: that parent may carry verbs other installed software
    /// registered, so removal stops at rDownloader's own `rDownloader.Import` below it. The
    /// exception is marked here, on the entry it applies to, rather than living as a second,
    /// differently shaped enumeration.
    pub(super) removes: String,
}

/// Every entry of one integration kind: the single source both sides move over.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn windows_entries(kind: Kind, executable: &str) -> Vec<RegistryEntry> {
    match kind {
        Kind::Association => windows_registry_entries(executable),
        Kind::Scheme => windows_scheme_entries(executable),
    }
}

/// The keys `remove` deletes, in the order they were declared and without repeats.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn windows_removal_keys(entries: &[RegistryEntry]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for entry in entries {
        if !keys.contains(&entry.removes) {
            keys.push(entry.removes.clone());
        }
    }
    keys
}

/// The full set of `HKCU\Software\Classes` registry entries `association install`
/// writes on Windows. Kept pure and cfg-free (no registry access, no `#[cfg(windows)]`)
/// so it can be unit-tested on any host; `windows_install` is the only non-test consumer,
/// which is why non-Windows builds see it as unused outside `#[cfg(test)]`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn windows_registry_entries(executable: &str) -> Vec<RegistryEntry> {
    let extension = r"HKCU\Software\Classes\.nzb".to_owned();
    let file_type = r"HKCU\Software\Classes\rDownloader.NZB".to_owned();
    let notification_sender = format!(r"HKCU\Software\Classes\AppUserModelId\{WINDOWS_APP_ID}");
    let mut entries = vec![
        RegistryEntry {
            key: extension.clone(),
            name: None,
            value: "rDownloader.NZB".to_owned(),
            removes: extension.clone(),
        },
        RegistryEntry {
            key: extension.clone(),
            name: Some("Content Type"),
            value: "application/x-nzb".to_owned(),
            removes: extension,
        },
        RegistryEntry {
            key: file_type.clone(),
            name: None,
            value: "NZB Usenet file".to_owned(),
            removes: file_type.clone(),
        },
        RegistryEntry {
            key: format!(r"{file_type}\shell\open\command"),
            name: None,
            value: format!("\"{executable}\" open \"%1\""),
            removes: file_type,
        },
        // Names the sender of the agent's desktop notifications. An unpackaged executable has
        // no AppUserModelID of its own, so without this the toast arrives as PowerShell.
        RegistryEntry {
            key: notification_sender.clone(),
            name: Some("DisplayName"),
            value: "rDownloader Capture".to_owned(),
            removes: notification_sender,
        },
    ];
    // "Import into rDownloader" on every type the agent takes, the NZB's included.
    for (extension, _) in HANDED_OVER {
        entries.extend(import_verb(extension, executable));
    }
    entries.extend(offered_entries(executable));
    entries
}

/// "Import into rDownloader" in the context menu of one file type, whichever program is its
/// default.
///
/// Windows 11 files this verb under "Show more options" (Shift+F10) rather than the top-level
/// context menu; top-level placement would require an IExplorerCommand handler plus sparse MSIX
/// signing, which is deliberately out of scope here. The removal stops at the verb and not at the
/// `SystemFileAssociations\<extension>` above it: that key is shared with whatever else registers
/// a verb for the file type.
fn import_verb(extension: &str, executable: &str) -> [RegistryEntry; 3] {
    let verb = format!(
        r"HKCU\Software\Classes\SystemFileAssociations\{extension}\shell\rDownloader.Import"
    );
    [
        RegistryEntry {
            key: verb.clone(),
            name: None,
            value: "Import into rDownloader".to_owned(),
            removes: verb.clone(),
        },
        RegistryEntry {
            key: verb.clone(),
            name: Some("Icon"),
            value: format!("\"{executable}\",0"),
            removes: verb.clone(),
        },
        RegistryEntry {
            key: format!(r"{verb}\command"),
            name: None,
            value: format!("\"{executable}\" open \"%1\""),
            removes: verb,
        },
    ]
}

/// The agent in "Open with" for the link containers and `.torrent` (RD-1260-09), without
/// claiming any of them: `Applications\rdownloader-capture.exe` with the types it supports. A
/// type no program claims offers it on a double-click, and a type another program owns stays
/// that program's -- nothing is written under `.dlc` or `.torrent` themselves, so no default is
/// displaced and the one key removes all of it. Only the NZB keeps the default it always had.
fn offered_entries(executable: &str) -> Vec<RegistryEntry> {
    let application = format!(r"HKCU\Software\Classes\Applications\{EXECUTABLE_NAME}");
    let mut entries = vec![
        RegistryEntry {
            key: application.clone(),
            name: Some("FriendlyAppName"),
            value: "rDownloader".to_owned(),
            removes: application.clone(),
        },
        RegistryEntry {
            key: format!(r"{application}\shell\open\command"),
            name: None,
            value: format!("\"{executable}\" open \"%1\""),
            removes: application.clone(),
        },
    ];
    for (extension, kind) in HANDED_OVER {
        if kind == FileKind::Nzb {
            continue;
        }
        entries.push(RegistryEntry {
            key: format!(r"{application}\SupportedTypes"),
            name: Some(extension),
            value: String::new(),
            removes: application.clone(),
        });
    }
    entries
}

/// The `HKCU\Software\Classes` entries that register the URL scheme on Windows.
///
/// Kept pure and cfg-free for the same reason as [`windows_registry_entries`]: the exact
/// quoting of the command is what decides whether an address with a space in it arrives
/// intact, and that has to be testable on any host.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn windows_scheme_entries(executable: &str) -> Vec<RegistryEntry> {
    // One branch, so one removal key for all four -- the scheme side never had the problem the
    // association side had, and moving it onto the same list costs nothing.
    let scheme = r"HKCU\Software\Classes\rdownloader".to_owned();
    vec![
        RegistryEntry {
            key: scheme.clone(),
            name: None,
            value: "URL:rDownloader Protocol".to_owned(),
            removes: scheme.clone(),
        },
        RegistryEntry {
            key: scheme.clone(),
            name: Some("URL Protocol"),
            value: String::new(),
            removes: scheme.clone(),
        },
        RegistryEntry {
            key: format!(r"{scheme}\DefaultIcon"),
            name: None,
            value: format!("\"{executable}\",0"),
            removes: scheme.clone(),
        },
        RegistryEntry {
            key: format!(r"{scheme}\shell\open\command"),
            name: None,
            value: format!("\"{executable}\" handle \"%1\""),
            removes: scheme,
        },
    ]
}
