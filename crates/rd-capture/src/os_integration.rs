use std::path::Path;

use anyhow::Result;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use anyhow::{Context, bail};
// The process and registry helpers are `rd-autostart`'s, which this crate already depends on;
// the copies that lived here are gone (audit 1.9.1, INTAKE-13).
#[cfg(target_os = "linux")]
use rd_autostart::shell::run;
#[cfg(windows)]
use rd_autostart::shell::{reg_add, reg_delete_key_if_present};

// The registry lists, testable on every host; written and removed only by the Windows functions
// below.
#[path = "os_integration_windows.rs"]
mod registry;

#[cfg(test)]
use registry::{RegistryEntry, windows_registry_entries, windows_scheme_entries};
#[cfg(any(windows, test))]
use registry::{windows_entries, windows_removal_keys};

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Association,
    /// The `rdownloader://` URL scheme handler (RD-090-09).
    Scheme,
}

#[cfg(windows)]
pub(crate) fn install(kind: Kind, executable: &Path) -> Result<()> {
    windows_install(kind, executable)
}

#[cfg(target_os = "linux")]
pub(crate) fn install(kind: Kind, executable: &Path) -> Result<()> {
    linux_install(kind, executable)
}

#[cfg(target_os = "macos")]
pub(crate) fn install(kind: Kind, executable: &Path) -> Result<()> {
    macos_install(kind, executable)
}

#[cfg(windows)]
pub(crate) fn remove(kind: Kind) -> Result<()> {
    windows_remove(kind)
}

#[cfg(target_os = "linux")]
pub(crate) fn remove(kind: Kind) -> Result<()> {
    linux_remove(kind)
}

#[cfg(target_os = "macos")]
pub(crate) fn remove(kind: Kind) -> Result<()> {
    macos_remove(kind)
}

/// The Windows AppUserModelID desktop notifications are shown under.
///
/// Without a registered one, a toast from an unpackaged executable is attributed to whatever
/// host process raised it — PowerShell, in practice. `notify.rs` sets the same identifier.
/// Defined unconditionally so `windows_registry_entries` stays testable on every host.
pub(crate) const WINDOWS_APP_ID: &str = "rDownloader.Capture";

#[cfg(windows)]
fn windows_install(kind: Kind, executable: &Path) -> Result<()> {
    let executable = executable.to_string_lossy();
    for entry in windows_entries(kind, &executable) {
        reg_add(&entry.key, entry.name, &entry.value)?;
    }
    Ok(())
}

#[cfg(windows)]
fn windows_remove(kind: Kind) -> Result<()> {
    // The same list `install` writes, read for its removal keys. The executable path plays no
    // part in a key, so an empty one is enough to build the entries here.
    for key in windows_removal_keys(&windows_entries(kind, "")) {
        reg_delete_key_if_present(&key)?;
    }
    Ok(())
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
            // The link containers' types (RD-1260-09): offered in "Open with" through the
            // desktop entry's `MimeType`, never made their default.
            std::fs::write(
                packages.join(LINUX_CONTAINER_MIME_PACKAGE),
                LINUX_CONTAINER_MIME_XML,
            )?;
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

/// The link containers' MIME package (RD-1260-09). `.torrent` needs none: shared-mime-info
/// knows `application/x-bittorrent`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_CONTAINER_MIME_PACKAGE: &str = "rdownloader-containers.xml";

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_CONTAINER_MIME_XML: &str = "<?xml version=\"1.0\"?><mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\"><mime-type type=\"application/x-dlc\"><comment>DLC link container</comment><glob pattern=\"*.dlc\"/></mime-type><mime-type type=\"application/x-ccf\"><comment>CCF link container</comment><glob pattern=\"*.ccf\"/></mime-type><mime-type type=\"application/x-rsdf\"><comment>RSDF link container</comment><glob pattern=\"*.rsdf\"/></mime-type><mime-type type=\"application/x-rdlinks\"><comment>rDownloader link file</comment><glob pattern=\"*.rdlinks\"/></mime-type><mime-type type=\"application/x-crawljob\"><comment>Crawljob link list</comment><glob pattern=\"*.crawljob\"/></mime-type></mime-info>";

/// Every MIME type the desktop entry offers the agent for: the NZB it is the default of, and the
/// containers and the torrent it is only offered for.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const LINUX_MIME_TYPES: &str = "application/x-nzb;application/x-dlc;application/x-ccf;application/x-rsdf;application/x-rdlinks;application/x-crawljob;application/x-bittorrent;";

#[cfg(target_os = "linux")]
fn render_linux_desktop(executable: &Path) -> Result<String> {
    let executable = desktop_quote(executable)?;
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=rDownloader Capture\nComment=Import NZB files and link containers into rDownloader\nNoDisplay=false\nTerminal=false\nExec={executable} open %F\nMimeType={LINUX_MIME_TYPES}\nCategories=Network;\n"
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
    if matches!(kind, Kind::Association) {
        let packages = base.data_local_dir().join("mime/packages");
        for package in ["rdownloader-nzb.xml", LINUX_CONTAINER_MIME_PACKAGE] {
            rd_autostart::shell::remove_file_if_present(&packages.join(package))?;
        }
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

#[cfg(target_os = "macos")]
const MACOS_HELPER_NAME: &str = "rDownloader Capture.app";

#[cfg(target_os = "macos")]
fn macos_install(kind: Kind, executable: &Path) -> Result<()> {
    let helper = macos_helper(executable)?;
    let info = std::fs::read_to_string(helper.join("Contents/Info.plist")).with_context(|| {
        format!(
            "macOS helper was not found at {}; keep `{MACOS_HELPER_NAME}` next to rdownloader-capture",
            helper.display()
        )
    })?;
    match kind {
        Kind::Association => {
            run_macos_launch_services("-f", &helper, "register macOS NZB association")
        }
        // A URL scheme on macOS is claimed by `CFBundleURLTypes` in an app bundle's Info.plist
        // (RD-180-05); Launch Services reads it when the bundle is registered. A helper from
        // before 1.8 declares none, and registering it would appear to succeed and then never
        // receive a link.
        Kind::Scheme => {
            if !declares_url_scheme(&info) {
                bail!(
                    "{} declares no rdownloader:// scheme; replace `{MACOS_HELPER_NAME}` with the one from this release",
                    helper.display()
                );
            }
            run_macos_launch_services("-f", &helper, "register the rdownloader:// scheme")
        }
    }
}

/// One registration of the helper carries both the NZB file type and the URL scheme, so
/// removing either unregisters the helper; `association install` or `scheme install` registers
/// both again.
#[cfg(target_os = "macos")]
fn macos_remove(_kind: Kind) -> Result<()> {
    // The path `install` registered, which is the package manager's stable alias.
    let executable = rd_autostart::stable_executable_path(
        &std::env::current_exe().context("locate capture executable")?,
    );
    let helper = macos_helper(&executable)?;
    if helper.exists() {
        run_macos_launch_services("-u", &helper, "unregister the macOS helper")?;
    }
    Ok(())
}

/// Whether a helper's Info.plist claims `rdownloader://`: a `CFBundleURLSchemes` entry naming
/// it. A text check, not a plist parser: the file is the one this repository ships, and the
/// question is only whether it is the one from 1.8 on.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn declares_url_scheme(info_plist: &str) -> bool {
    info_plist
        .split_once("<key>CFBundleURLSchemes</key>")
        .and_then(|(_, rest)| rest.split_once("</array>"))
        .is_some_and(|(schemes, _)| schemes.contains("<string>rdownloader</string>"))
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
#[path = "os_integration_tests.rs"]
mod tests;
