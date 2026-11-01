//! The two drop targets on the Windows desktop (RD-1260-09): "Add to rDownloader" on the desktop
//! and "rDownloader" in the Send to menu. Both are shortcuts to `rdownloader-capture open`;
//! Windows passes the files dropped on a shortcut, or sent to one, as its arguments, so they end
//! where a double-click on an associated file ends.
//!
//! The icon in the notification area cannot be the target: it takes no dropped files, a limit of
//! Windows rather than of `tray-icon`.
//!
//! The shortcut is written through the shell's own `IShellLinkW` in this process
//! (`rd_win32::write_shortcut`, RD-1400-07; before, a PowerShell helper did it through
//! `WScript.Shell`). What the shortcut is -- folder, name, target, arguments -- is decided here and
//! tested on every host.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Where a shortcut lies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Place {
    /// The desktop: "Add to rDownloader", files dropped on it.
    Desktop,
    /// The Send to menu of every file's context menu.
    SendTo,
}

impl Place {
    /// The shortcut's file; Windows shows its name without `.lnk`.
    pub(crate) fn file_name(self) -> &'static str {
        match self {
            Self::Desktop => "Add to rDownloader.lnk",
            Self::SendTo => "rDownloader.lnk",
        }
    }

    /// What `install` and `remove` say they did.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Desktop => "desktop shortcut \"Add to rDownloader\"",
            Self::SendTo => "Send to entry \"rDownloader\"",
        }
    }

    /// The shell folder it lies in.
    #[cfg(windows)]
    fn known_folder(self) -> rd_win32::KnownFolder {
        match self {
            Self::Desktop => rd_win32::KnownFolder::Desktop,
            Self::SendTo => rd_win32::KnownFolder::SendTo,
        }
    }
}

/// The arguments the shortcut starts the agent with; the files follow them.
pub(crate) const ARGUMENTS: &str = "open";

/// The shortcut's tooltip.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) const DESCRIPTION: &str = "Hands the files dropped on it to rDownloader";

/// What the shortcut to the agent at `executable` holds: the agent with [`ARGUMENTS`], started in
/// its own folder, with its own icon, minimised.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Link {
    pub target: PathBuf,
    pub arguments: &'static str,
    pub working_directory: PathBuf,
    pub icon: PathBuf,
    pub description: &'static str,
    pub minimized: bool,
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn link(executable: &Path) -> Link {
    Link {
        target: executable.to_path_buf(),
        arguments: ARGUMENTS,
        working_directory: executable
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
        icon: executable.to_path_buf(),
        description: DESCRIPTION,
        minimized: true,
    }
}

/// Puts the shortcut in its place, for the agent at `executable`.
pub(crate) fn install(place: Place, executable: &Path) -> Result<()> {
    write_in(&folder(place)?, place, executable)
}

/// Takes the shortcut away again; a folder the shell does not have holds none.
pub(crate) fn remove(place: Place) -> Result<()> {
    match folder(place) {
        Ok(folder) => remove_in(&folder, place),
        Err(_) if cfg!(windows) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn folder(place: Place) -> Result<PathBuf> {
    rd_win32::known_folder(place.known_folder())
        .with_context(|| format!("find the folder of the {}", place.label()))
}

#[cfg(not(windows))]
fn folder(place: Place) -> Result<PathBuf> {
    anyhow::bail!("the {} exists on Windows only", place.label())
}

/// Writes the shortcut for `place` into `folder` (the tests' own folder, or the shell's).
#[cfg(windows)]
fn write_in(folder: &Path, place: Place, executable: &Path) -> Result<()> {
    let link = link(executable);
    rd_win32::write_shortcut(
        &folder.join(place.file_name()),
        &rd_win32::Shortcut {
            target: &link.target,
            arguments: link.arguments,
            working_directory: &link.working_directory,
            icon: &link.icon,
            description: link.description,
            minimized: link.minimized,
        },
    )
    .with_context(|| format!("write the {}", place.label()))
}

#[cfg(not(windows))]
fn write_in(_folder: &Path, place: Place, _executable: &Path) -> Result<()> {
    anyhow::bail!("the {} exists on Windows only", place.label())
}

/// Removes the shortcut for `place` from `folder`; one that is not there is removed already.
fn remove_in(folder: &Path, place: Place) -> Result<()> {
    match std::fs::remove_file(folder.join(place.file_name())) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("remove the {}", place.label()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "desktop_shortcut_tests.rs"]
mod tests;
