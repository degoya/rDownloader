//! The two drop targets on the Windows desktop (RD-1260-09): "Add to rDownloader" on the desktop
//! and "rDownloader" in the Send to menu. Both are shortcuts to `rdownloader-capture open`;
//! Windows passes the files dropped on a shortcut, or sent to one, as its arguments, so they end
//! where a double-click on an associated file ends.
//!
//! The icon in the notification area cannot be the target: it takes no dropped files, a limit of
//! Windows rather than of `tray-icon`.
//!
//! The workspace denies `unsafe_code`, so the shortcut is not written through COM from this
//! process. A PowerShell helper writes it through `WScript.Shell`, as `rd_power` and the game mode
//! ask Windows through one; every value reaches it through the environment, never spliced into
//! the script. What the shortcut is -- folder, name, arguments -- is decided here and tested on
//! every host.

use std::path::Path;

use anyhow::Result;

/// Where a shortcut lies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Place {
    /// The desktop: "Add to rDownloader", files dropped on it.
    Desktop,
    /// The Send to menu of every file's context menu.
    SendTo,
}

impl Place {
    /// The folder, as `[Environment]::GetFolderPath` names it.
    pub(crate) fn folder(self) -> &'static str {
        match self {
            Self::Desktop => "DesktopDirectory",
            Self::SendTo => "SendTo",
        }
    }

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
}

/// The arguments the shortcut starts the agent with; the files follow them.
pub(crate) const ARGUMENTS: &str = "open";

/// Writes the shortcut. `RD_SHORTCUT_FOLDER` is a folder name of `GetFolderPath`, or a full path
/// (the tests' own folder). The console of `open` starts minimised (`WindowStyle` 7).
pub(crate) const CREATE_SCRIPT: &str = r"$ErrorActionPreference = 'Stop'
$folder = $env:RD_SHORTCUT_FOLDER
if (-not [IO.Path]::IsPathRooted($folder)) { $folder = [Environment]::GetFolderPath($folder) }
if (-not $folder) { throw 'the folder for the shortcut does not exist' }
$link = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $folder $env:RD_SHORTCUT_NAME))
$link.TargetPath = $env:RD_SHORTCUT_TARGET
$link.Arguments = $env:RD_SHORTCUT_ARGUMENTS
$link.WorkingDirectory = Split-Path -Parent $env:RD_SHORTCUT_TARGET
$link.IconLocation = $env:RD_SHORTCUT_TARGET + ',0'
$link.Description = 'Hands the files dropped on it to rDownloader'
$link.WindowStyle = 7
$link.Save()";

/// Removes the shortcut; one that is not there is removed already.
pub(crate) const REMOVE_SCRIPT: &str = r"$ErrorActionPreference = 'Stop'
$folder = $env:RD_SHORTCUT_FOLDER
if (-not [IO.Path]::IsPathRooted($folder)) { $folder = [Environment]::GetFolderPath($folder) }
if (-not $folder) { exit 0 }
$path = Join-Path $folder $env:RD_SHORTCUT_NAME
if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }";

/// The environment both scripts read, for `place` in `folder` and the agent at `executable`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn environment(
    place: Place,
    folder: &str,
    executable: &Path,
) -> Vec<(&'static str, String)> {
    vec![
        ("RD_SHORTCUT_FOLDER", folder.to_owned()),
        ("RD_SHORTCUT_NAME", place.file_name().to_owned()),
        (
            "RD_SHORTCUT_TARGET",
            executable.to_string_lossy().into_owned(),
        ),
        ("RD_SHORTCUT_ARGUMENTS", ARGUMENTS.to_owned()),
    ]
}

/// Puts the shortcut in its place, for the agent at `executable`.
pub(crate) fn install(place: Place, executable: &Path) -> Result<()> {
    run(CREATE_SCRIPT, "write", place, place.folder(), executable)
}

/// Takes the shortcut away again.
pub(crate) fn remove(place: Place) -> Result<()> {
    run(
        REMOVE_SCRIPT,
        "remove",
        place,
        place.folder(),
        Path::new(""),
    )
}

/// Runs one of the scripts for `place` in `folder`; `verb` says what it does, for the error.
#[cfg(windows)]
fn run(script: &str, verb: &str, place: Place, folder: &str, executable: &Path) -> Result<()> {
    use rd_files::NoConsoleWindow as _;

    let mut command = std::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .envs(environment(place, folder, executable))
        .no_console_window();
    rd_autostart::shell::run(&mut command, &format!("{verb} the {}", place.label()))
}

#[cfg(not(windows))]
fn run(_script: &str, _verb: &str, place: Place, _folder: &str, _executable: &Path) -> Result<()> {
    anyhow::bail!("the {} exists on Windows only", place.label())
}

#[cfg(test)]
#[path = "desktop_shortcut_tests.rs"]
mod tests;
