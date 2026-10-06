//! Per-user autostart registration for the rDownloader binaries, and where an installed build
//! keeps its data.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

mod installed;
pub mod shell;
mod stable_path;

pub use installed::{INSTALL_KIND_FILE, install_kind, installed_home};
pub use stable_path::stable_executable_path;

/// One independently installable rDownloader background process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Server,
    Capture,
}

impl Target {
    fn slug(self) -> &'static str {
        match self {
            Self::Server => "rdownloader",
            Self::Capture => "rdownloader-capture",
        }
    }

    /// The name shown in the Windows Run key and the systemd unit's `Description`; a launchd
    /// agent has no such field, so macOS does without it.
    #[cfg(any(windows, target_os = "linux"))]
    fn display_name(self) -> &'static str {
        match self {
            Self::Server => "rDownloader Service",
            Self::Capture => "rDownloader Capture",
        }
    }

    fn argument(self) -> &'static str {
        match self {
            Self::Server => "serve",
            Self::Capture => "run",
        }
    }

    #[cfg(target_os = "macos")]
    fn launchd_label(self) -> &'static str {
        match self {
            Self::Server => "org.rdownloader.service",
            Self::Capture => "org.rdownloader.capture",
        }
    }

    #[cfg(target_os = "linux")]
    fn wanted_by(self) -> &'static str {
        match self {
            Self::Server => "default.target",
            Self::Capture => "graphical-session.target",
        }
    }
}

struct Registration {
    target: Target,
    executable: PathBuf,
    working_directory: PathBuf,
    log_directory: PathBuf,
}

impl Registration {
    fn new(target: Target, executable: &Path) -> Result<Self> {
        if !executable.is_absolute() {
            bail!("autostart executable must be an absolute path");
        }
        // An installed build starts in the user's data folder, as its service would move there
        // anyway; its logs must not land in a program folder it may not write (RD-180-05).
        let working_directory = match installed_home(executable)? {
            Some(home) => home,
            None => executable
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .context("autostart executable has no parent directory")?
                .to_owned(),
        };
        let log_directory = working_directory.join("logs");
        std::fs::create_dir_all(&log_directory)
            .with_context(|| format!("create {}", log_directory.display()))?;
        Ok(Self {
            target,
            executable: executable.to_owned(),
            working_directory,
            log_directory,
        })
    }

    fn stdout_path(&self) -> PathBuf {
        self.log_directory
            .join(format!("{}.log", self.target.slug()))
    }

    fn stderr_path(&self) -> PathBuf {
        self.log_directory
            .join(format!("{}.err.log", self.target.slug()))
    }
}

/// Registers `executable` to start for the current user at the next login.
///
/// Registered under [`stable_executable_path`], so a package manager's update does not leave the
/// login entry pointing at the previous version's folder.
pub fn install(target: Target, executable: &Path) -> Result<()> {
    let registration = Registration::new(target, &stable_executable_path(executable))?;
    platform::install(&registration)
}

/// Removes the current user's registration without stopping a running process.
pub fn remove(target: Target) -> Result<()> {
    platform::remove(target)
}

/// Whether `target` has its `Run` entry for the next login. The updater asks before it takes a
/// failed MSI update back (RD-180-02): removing the newer package removes the entry too, and the
/// reinstalled previous one only gets it back when it is registered again.
#[cfg(windows)]
#[must_use]
pub fn is_registered(target: Target) -> bool {
    platform::is_registered(target)
}

/// The text of a Windows autostart wrapper path, refused when it carries a control character.
///
/// A line break in the path would close the `shell.Run` line of the generated VBScript and turn
/// everything after it into a statement of its own, and the registry value is a command line
/// that `wscript.exe` is handed verbatim. The path is derived from the install location today,
/// so nothing reaches this from outside — but the systemd renderer rejects the same characters
/// and the property-list renderer rejects `\0`, and a renderer that trusts its input is the one
/// that stops being safe when somebody changes where the wrapper lives.
///
/// Ungated so it compiles and is tested on every platform; only the renderers that use it are
/// Windows-only.
#[cfg(any(windows, test))]
fn windows_wrapper_path(path: &Path) -> Result<&str> {
    let value = path
        .to_str()
        .context("autostart wrapper path is not Unicode")?;
    if value.contains(['\n', '\r', '\0']) {
        bail!("autostart path cannot be represented in a Windows autostart wrapper");
    }
    Ok(value)
}

#[cfg(test)]
mod tests;

#[cfg(windows)]
#[path = "platform_windows.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "platform_linux.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "platform_macos.rs"]
mod platform;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod platform {
    use anyhow::{Result, bail};

    use super::{Registration, Target};

    pub(super) fn install(_registration: &Registration) -> Result<()> {
        bail!("autostart is not supported on this operating system")
    }

    pub(super) fn remove(_target: Target) -> Result<()> {
        bail!("autostart is not supported on this operating system")
    }
}
