use std::path::Path;

use anyhow::{Context, Result, bail};

#[derive(Clone, Copy)]
pub enum Kind {
    Association,
    /// The `rdownloader://` URL scheme handler (RD-090-09).
    Scheme,
}

#[cfg(windows)]
pub fn install(kind: Kind, executable: &Path) -> Result<()> {
    windows_install(kind, executable)
}

#[cfg(target_os = "linux")]
pub fn install(kind: Kind, executable: &Path) -> Result<()> {
    linux_install(kind, executable)
}

#[cfg(target_os = "macos")]
pub fn install(kind: Kind, executable: &Path) -> Result<()> {
    macos_install(kind, executable)
}

#[cfg(windows)]
pub fn remove(kind: Kind) -> Result<()> {
    windows_remove(kind)
}

#[cfg(target_os = "linux")]
pub fn remove(kind: Kind) -> Result<()> {
    linux_remove(kind)
}

#[cfg(target_os = "macos")]
pub fn remove(kind: Kind) -> Result<()> {
    macos_remove(kind)
}

/// The Windows AppUserModelID desktop notifications are shown under.
///
/// Without a registered one, a toast from an unpackaged executable is attributed to whatever
/// host process raised it — PowerShell, in practice. `notify.rs` sets the same identifier.
/// Defined unconditionally so `windows_registry_entries` stays testable on every host.
pub const WINDOWS_APP_ID: &str = "rDownloader.Capture";

#[cfg(windows)]
fn windows_install(kind: Kind, executable: &Path) -> Result<()> {
    let executable = executable.to_string_lossy();
    for entry in windows_entries(kind, &executable) {
        reg_add(&entry.key, entry.name, &entry.value)?;
    }
    Ok(())
}

/// One `HKCU\Software\Classes` entry the capture agent writes, and how far removing it may go.
///
/// Installing and removing used to be two independent enumerations -- a list of tuples on one
/// side, a sequence of calls on the other -- with nothing connecting them, so a key added to one
/// side and not the other went unnoticed. That is exactly what happened to the
/// `AppUserModelId` entry, which `association install` wrote and `association remove` left
/// behind for good (RD-109-15).
#[cfg_attr(not(windows), allow(dead_code))]
struct RegistryEntry {
    key: String,
    /// The value name, or `None` for the key's default value.
    name: Option<&'static str>,
    value: String,
    /// The key `remove` deletes for this entry, with everything under it.
    ///
    /// Usually the top of the entry's own branch, so several entries under one branch collapse
    /// to a single delete. For the `SystemFileAssociations\.nzb` entries it is deliberately
    /// *not* the top of the branch: that parent may carry verbs other installed software
    /// registered, so removal stops at rDownloader's own `rDownloader.Import` below it. The
    /// exception is marked here, on the entry it applies to, rather than living as a second,
    /// differently shaped enumeration.
    removes: String,
}

/// Every entry of one integration kind: the single source both sides move over.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_entries(kind: Kind, executable: &str) -> Vec<RegistryEntry> {
    match kind {
        Kind::Association => windows_registry_entries(executable),
        Kind::Scheme => windows_scheme_entries(executable),
    }
}

/// The keys `remove` deletes, in the order they were declared and without repeats.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_removal_keys(entries: &[RegistryEntry]) -> Vec<String> {
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
fn windows_registry_entries(executable: &str) -> Vec<RegistryEntry> {
    let extension = r"HKCU\Software\Classes\.nzb".to_owned();
    let file_type = r"HKCU\Software\Classes\rDownloader.NZB".to_owned();
    // The removal stops here and not at the `SystemFileAssociations\.nzb` above it: that key is
    // shared with whatever else registers a verb for the file type.
    let import_verb =
        r"HKCU\Software\Classes\SystemFileAssociations\.nzb\shell\rDownloader.Import".to_owned();
    let notification_sender = format!(r"HKCU\Software\Classes\AppUserModelId\{WINDOWS_APP_ID}");
    vec![
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
        // Windows 11 files this verb under "Show more options" (Shift+F10) rather than the
        // top-level context menu; top-level placement would require an IExplorerCommand
        // handler plus sparse MSIX signing, which is deliberately out of scope here.
        RegistryEntry {
            key: import_verb.clone(),
            name: None,
            value: "Import into rDownloader".to_owned(),
            removes: import_verb.clone(),
        },
        RegistryEntry {
            key: import_verb.clone(),
            name: Some("Icon"),
            value: format!("\"{executable}\",0"),
            removes: import_verb.clone(),
        },
        RegistryEntry {
            key: format!(r"{import_verb}\command"),
            name: None,
            value: format!("\"{executable}\" open \"%1\""),
            removes: import_verb,
        },
        // Names the sender of the agent's desktop notifications. An unpackaged executable has
        // no AppUserModelID of its own, so without this the toast arrives as PowerShell.
        RegistryEntry {
            key: notification_sender.clone(),
            name: Some("DisplayName"),
            value: "rDownloader Capture".to_owned(),
            removes: notification_sender,
        },
    ]
}

#[cfg(windows)]
fn windows_remove(kind: Kind) -> Result<()> {
    // The same list `install` writes, read for its removal keys. The executable path plays no
    // part in a key, so an empty one is enough to build the entries here.
    for key in windows_removal_keys(&windows_entries(kind, "")) {
        reg_delete_tree_if_present(&key)?;
    }
    Ok(())
}

/// The `HKCU\Software\Classes` entries that register the URL scheme on Windows.
///
/// Kept pure and cfg-free for the same reason as [`windows_registry_entries`]: the exact
/// quoting of the command is what decides whether an address with a space in it arrives
/// intact, and that has to be testable on any host.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_scheme_entries(executable: &str) -> Vec<RegistryEntry> {
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

#[cfg(windows)]
fn reg_add(key: &str, name: Option<&str>, value: &str) -> Result<()> {
    let mut command = std::process::Command::new("reg.exe");
    command.args(["add", key]);
    if let Some(name) = name {
        command.args(["/v", name]);
    } else {
        command.arg("/ve");
    }
    command.args(["/t", "REG_SZ", "/d", value, "/f"]);
    run(&mut command, "write Windows registry")
}

#[cfg(windows)]
fn reg_delete_tree_if_present(key: &str) -> Result<()> {
    let present = std::process::Command::new("reg.exe")
        .args(["query", key])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("query Windows registry key {key}"))?
        .success();
    if !present {
        return Ok(());
    }
    run(
        std::process::Command::new("reg.exe").args(["delete", key, "/f"]),
        "remove Windows registry key",
    )
}

#[cfg(target_os = "linux")]
fn linux_install(kind: Kind, executable: &Path) -> Result<()> {
    let base = directories::BaseDirs::new().context("locate user data directory")?;
    match kind {
        Kind::Association => {
            let applications = base.data_local_dir().join("applications");
            std::fs::create_dir_all(&applications)?;
            std::fs::write(
                applications.join("rdownloader-capture.desktop"),
                render_linux_desktop(executable)?,
            )?;
            let packages = base.data_local_dir().join("mime/packages");
            std::fs::create_dir_all(&packages)?;
            std::fs::write(packages.join("rdownloader-nzb.xml"), LINUX_NZB_MIME_XML)?;
            let _ = std::process::Command::new("update-mime-database")
                .arg(base.data_local_dir().join("mime"))
                .status();
            let _ = std::process::Command::new("update-desktop-database")
                .arg(&applications)
                .status();
            run(
                std::process::Command::new("xdg-mime").args([
                    "default",
                    "rdownloader-capture.desktop",
                    "application/x-nzb",
                ]),
                "set the default NZB file association (install xdg-utils if xdg-mime is missing)",
            )?;
        }
        Kind::Scheme => {
            let applications = base.data_local_dir().join("applications");
            std::fs::create_dir_all(&applications)?;
            std::fs::write(
                applications.join("rdownloader-scheme.desktop"),
                render_linux_scheme_desktop(executable)?,
            )?;
            let _ = std::process::Command::new("update-desktop-database")
                .arg(&applications)
                .status();
            run(
                std::process::Command::new("xdg-mime").args([
                    "default",
                    "rdownloader-scheme.desktop",
                    "x-scheme-handler/rdownloader",
                ]),
                "register the rdownloader:// scheme (install xdg-utils if xdg-mime is missing)",
            )?;
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
const LINUX_NZB_MIME_XML: &str = "<?xml version=\"1.0\"?><mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\"><mime-type type=\"application/x-nzb\"><comment>NZB Usenet file</comment><glob pattern=\"*.nzb\"/></mime-type></mime-info>";

#[cfg(target_os = "linux")]
fn render_linux_desktop(executable: &Path) -> Result<String> {
    let executable = desktop_quote(executable)?;
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=rDownloader Capture\nComment=Import NZB into rDownloader\nNoDisplay=false\nTerminal=false\nExec={executable} open %f\nMimeType=application/x-nzb;\nCategories=Network;\n"
    ))
}

/// The desktop entry that claims `x-scheme-handler/rdownloader`.
///
/// `NoDisplay=true`: this entry exists to be found by the desktop's URL dispatcher, not to
/// appear in an application menu as a second, confusing "rDownloader Capture".
#[cfg(target_os = "linux")]
fn render_linux_scheme_desktop(executable: &Path) -> Result<String> {
    let executable = desktop_quote(executable)?;
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=rDownloader URL handler\nComment=Hand rdownloader:// links to rDownloader\nNoDisplay=true\nTerminal=false\nExec={executable} handle %u\nMimeType=x-scheme-handler/rdownloader;\nCategories=Network;\n"
    ))
}

#[cfg(target_os = "linux")]
fn linux_remove(kind: Kind) -> Result<()> {
    let base = directories::BaseDirs::new().context("locate user data directory")?;
    let path = match kind {
        Kind::Association => base
            .data_local_dir()
            .join("applications/rdownloader-capture.desktop"),
        Kind::Scheme => base
            .data_local_dir()
            .join("applications/rdownloader-scheme.desktop"),
    };
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    // The NZB mime package belongs to the file association only; removing the scheme
    // handler must not take the file type with it.
    let mime_package = base
        .data_local_dir()
        .join("mime/packages/rdownloader-nzb.xml");
    if matches!(kind, Kind::Association) && mime_package.exists() {
        std::fs::remove_file(mime_package)?;
    }
    let _ = std::process::Command::new("update-mime-database")
        .arg(base.data_local_dir().join("mime"))
        .status();
    let _ = std::process::Command::new("update-desktop-database")
        .arg(base.data_local_dir().join("applications"))
        .status();
    Ok(())
}

#[cfg(target_os = "linux")]
fn desktop_quote(path: &Path) -> Result<String> {
    let path = path
        .to_str()
        .context("capture executable path is not Unicode")?;
    if path.contains(['\n', '\r']) {
        bail!("capture executable path cannot be represented in a desktop entry");
    }
    Ok(format!(
        "\"{}\"",
        path.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%")
    ))
}

#[cfg(any(windows, target_os = "linux"))]
fn run(command: &mut std::process::Command, operation: &str) -> Result<()> {
    let status = command.status().with_context(|| operation.to_owned())?;
    if !status.success() {
        bail!("{operation} failed with {status}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
const MACOS_HELPER_NAME: &str = "rDownloader Capture.app";

#[cfg(target_os = "macos")]
fn macos_install(kind: Kind, executable: &Path) -> Result<()> {
    match kind {
        Kind::Association => {
            let helper = macos_helper(executable)?;
            if !helper.join("Contents/Info.plist").is_file() {
                bail!(
                    "macOS NZB helper was not found at {}; keep `{MACOS_HELPER_NAME}` next to rdownloader-capture",
                    helper.display()
                );
            }
            run_macos_launch_services("-f", &helper, "register macOS NZB association")
        }
        // A URL scheme on macOS is claimed by `CFBundleURLTypes` in an app bundle's
        // Info.plist; a bare executable cannot register one at all. Saying so is better
        // than a call that appears to succeed and then never receives a link.
        Kind::Scheme => bail!(
            "registering a URL scheme on macOS requires an application bundle; \
             `{MACOS_HELPER_NAME}` would have to declare CFBundleURLTypes"
        ),
    }
}

#[cfg(target_os = "macos")]
fn macos_remove(kind: Kind) -> Result<()> {
    match kind {
        Kind::Association => {
            let executable = std::env::current_exe().context("locate capture executable")?;
            let helper = macos_helper(&executable)?;
            if helper.exists() {
                run_macos_launch_services("-u", &helper, "unregister macOS NZB association")?;
            }
            Ok(())
        }
        // Nothing was ever registered, so removing is a no-op rather than an error.
        Kind::Scheme => Ok(()),
    }
}

#[cfg(target_os = "macos")]
fn macos_helper(executable: &Path) -> Result<std::path::PathBuf> {
    executable
        .parent()
        .map(|directory| directory.join(MACOS_HELPER_NAME))
        .context("capture executable has no parent directory")
}

#[cfg(target_os = "macos")]
fn run_macos_launch_services(action: &str, helper: &Path, operation: &str) -> Result<()> {
    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
    let status = std::process::Command::new(LSREGISTER)
        .args([action])
        .arg(helper)
        .status()
        .with_context(|| operation.to_owned())?;
    if !status.success() {
        bail!("{operation} failed with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_windows_scheme_entries_declare_a_protocol_and_quote_the_command() {
        let entries = super::windows_scheme_entries(r"C:\Tools\rdownloader-capture.exe");
        let rendered: Vec<String> = entries
            .iter()
            .map(|entry| {
                format!(
                    "{}|{}|{}",
                    entry.key,
                    entry.name.unwrap_or_default(),
                    entry.value
                )
            })
            .collect();
        let joined = rendered.join("\n");
        // Without the empty `URL Protocol` value Windows does not treat the key as a
        // scheme at all, and the handler is simply never invoked.
        assert!(joined.contains("URL Protocol"), "{joined}");
        // The command has to quote both the executable and `%1`: an address with a space
        // would otherwise arrive split across arguments.
        assert!(
            joined.contains("\"C:\\Tools\\rdownloader-capture.exe\" handle \"%1\""),
            "{joined}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_scheme_desktop_entry_claims_the_handler_without_showing_a_second_app() {
        let rendered = super::render_linux_scheme_desktop(std::path::Path::new("/opt/rd/capture"))
            .expect("render");
        assert!(
            rendered.contains("MimeType=x-scheme-handler/rdownloader;"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Exec=\"/opt/rd/capture\" handle %u"),
            "{rendered}"
        );
        // Two entries named rDownloader in the application menu is a worse outcome than
        // one; this one exists only for the URL dispatcher.
        assert!(rendered.contains("NoDisplay=true"), "{rendered}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_and_mime_outputs_cover_nzb_and_reserved_paths() {
        let desktop = super::render_linux_desktop(std::path::Path::new(
            "/tmp/Portable 100%/$Tools/\"capture\"",
        ))
        .expect("valid desktop path");
        assert!(desktop.contains(r#"Exec="/tmp/Portable 100%%/\$Tools/\"capture\"" open %f"#));
        assert!(desktop.contains("Terminal=false"));
        assert!(desktop.contains("NoDisplay=false"));
        assert!(desktop.contains("Comment=Import NZB into rDownloader"));
        assert!(desktop.contains("Categories=Network;"));
        assert!(desktop.contains("MimeType=application/x-nzb;"));
        assert!(super::LINUX_NZB_MIME_XML.contains("<glob pattern=\"*.nzb\"/>"));
    }

    #[test]
    fn windows_registry_entries_include_import_verb_with_exact_quoting() {
        let executable = r"C:\Program Files\rDownloader\rdownloader-capture.exe";
        let entries = super::windows_registry_entries(executable);

        assert_eq!(
            entries.len(),
            8,
            "expected 4 file-association entries, 3 Import verb entries and the notification \
             AppUserModelID"
        );
        assert!(
            entries.iter().any(|entry| entry.key
                == format!(
                    r"HKCU\Software\Classes\AppUserModelId\{}",
                    super::WINDOWS_APP_ID
                )
                && entry.name == Some("DisplayName")
                && entry.value == "rDownloader Capture"),
            "desktop notifications need a registered sender"
        );

        let import_key =
            r"HKCU\Software\Classes\SystemFileAssociations\.nzb\shell\rDownloader.Import";
        let import_command_key =
            r"HKCU\Software\Classes\SystemFileAssociations\.nzb\shell\rDownloader.Import\command";

        assert!(entries.iter().any(|entry| entry.key == import_key
            && entry.name.is_none()
            && entry.value == "Import into rDownloader"));

        assert!(entries.iter().any(|entry| entry.key == import_key
            && entry.name == Some("Icon")
            && entry.value == r#""C:\Program Files\rDownloader\rdownloader-capture.exe",0"#));

        assert!(entries.iter().any(|entry| entry.key == import_command_key
            && entry.name.is_none()
            && entry.value
                == r#""C:\Program Files\rDownloader\rdownloader-capture.exe" open "%1""#));
    }

    /// Install and remove move over one list, so a key cannot end up on only one side. What can
    /// still go wrong is an entry whose removal key does not actually cover it, and that is what
    /// this pins -- including a deliberately mismatched entry, to show the check bites
    /// (RD-109-15).
    #[test]
    fn every_entry_is_covered_by_the_key_that_removes_it() {
        let executable = r"C:\Tools\rdownloader-capture.exe";
        for kind in [super::Kind::Association, super::Kind::Scheme] {
            let entries = super::windows_entries(kind, executable);
            assert!(!entries.is_empty());
            for entry in &entries {
                assert!(
                    entry.key.starts_with(&entry.removes),
                    "{} is written but `remove` would never reach it: it deletes {}",
                    entry.key,
                    entry.removes
                );
            }
            // Every removal key belongs to an entry, and every entry to a removal key.
            let removal = super::windows_removal_keys(&entries);
            for key in &removal {
                assert!(
                    entries.iter().any(|entry| &entry.removes == key),
                    "{key} is deleted but nothing writes under it"
                );
            }
            for entry in &entries {
                assert!(
                    removal.contains(&entry.removes),
                    "{} is written and never deleted",
                    entry.key
                );
            }
        }

        // The same check against an entry that was added on the writing side only, in the way
        // the AppUserModelID once was: it names a branch no removal key reaches.
        let stray = super::RegistryEntry {
            key: r"HKCU\Software\Classes\rDownloader.Something".to_owned(),
            name: None,
            value: "left behind".to_owned(),
            removes: r"HKCU\Software\Classes\.nzb".to_owned(),
        };
        assert!(
            !stray.key.starts_with(&stray.removes),
            "the assertion above is what turns such an entry red"
        );
    }

    /// The notification sender is the key that was written and never removed.
    #[test]
    fn association_remove_takes_the_notification_sender_with_it() {
        let keys = super::windows_removal_keys(&super::windows_registry_entries("x"));
        assert!(
            keys.contains(&format!(
                r"HKCU\Software\Classes\AppUserModelId\{}",
                super::WINDOWS_APP_ID
            )),
            "{keys:?}"
        );
    }

    /// The deliberate restraint: rDownloader's own verb goes, the shared parent stays.
    #[test]
    fn the_shared_file_association_parent_is_never_deleted() {
        let parent = r"HKCU\Software\Classes\SystemFileAssociations\.nzb";
        let keys = super::windows_removal_keys(&super::windows_registry_entries("x"));
        for key in &keys {
            assert!(
                !parent.starts_with(key.as_str()),
                "{key} would take {parent} with it, and other software registers verbs there"
            );
        }
        assert!(
            keys.iter().any(|key| key
                == r"HKCU\Software\Classes\SystemFileAssociations\.nzb\shell\rDownloader.Import"),
            "rDownloader's own verb is still removed: {keys:?}"
        );
    }
}
