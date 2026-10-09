//! How this installation came to be, and therefore what its person does about a new version
//! (owner, 2026-09-30): a portable archive and the Windows installer install the update themselves
//! (RD-180-02, `crate::install`); anything a package manager or a container runtime owns is
//! updated by that tool, and the interface shows its command. The deb and rpm packages count as
//! the latter: installing them needs root, and a service that raised its own rights to do so
//! would be the opposite of the single-user default, so they show the apt and dnf command for
//! the project's repository (RD-180-10), and the downloaded package's way as a hint.
//!
//! Detection, first match wins:
//!
//! 1. **`RDOWNLOADER_INSTALL_KIND`** ([`INSTALL_KIND_ENV`]) names the kind outright. The image
//!    sets it (`docker/Dockerfile`), and it is the escape hatch for a layout nothing below knows.
//! 2. **A container** (`/.dockerenv`, `/run/.containerenv`): the image is replaced, never the
//!    binary inside it.
//! 3. **The executable's path** where a package manager's layout is unmistakable: Scoop's
//!    `apps/rdownloader/<version>/`, Homebrew's `Cellar/rdownloader/`, winget's
//!    `WinGet/Packages/`. These install the same archive a person unpacks by hand, so the path has
//!    to win over the archive's own marker.
//! 4. **The marker file** [`INSTALL_KIND_FILE`] (`install-kind`) beside the executable, or for an
//!    executable in a `bin` directory at `../lib/rdownloader/install-kind` (a distribution
//!    package's `/usr/bin/rdownloader` → `/usr/lib/rdownloader/install-kind`, for a symlink that
//!    does not resolve). The installers write it (RD-180-05: the MSI `msi`, deb and rpm
//!    `deb`/`rpm`, all in `/usr/lib/rdownloader/` or the MSI's folder beside the real
//!    executable; `rd_autostart::installed` reads the same file), the AUR package `aur`. Format:
//!    UTF-8 text, the first line that is neither empty nor a `#` comment is the kind, one of
//!    [`InstallKind::as_str`]; at most 1 KiB is read.
//! 5. **`VERSION.txt` beside the executable**: the portable archive, which carries no marker.
//! 6. Otherwise [`InstallKind::Unknown`]: a build from source, or a layout this build does not
//!    know. It is offered the download, never a command it may not have.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The marker file every package build writes beside the executable.
pub const INSTALL_KIND_FILE: &str = "install-kind";
/// The environment variable that names the kind outright.
pub const INSTALL_KIND_ENV: &str = "RDOWNLOADER_INSTALL_KIND";
/// What every release archive carries beside the executable (`scripts/lib/version-file.sh`).
const VERSION_FILE: &str = "VERSION.txt";
/// The most of a marker file that is read.
const MAX_MARKER_BYTES: u64 = 1024;

/// How this installation was installed.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallKind {
    /// The `.tar.gz`/`.zip` archive, unpacked by hand.
    Portable,
    /// The Windows installer.
    Msi,
    /// The Debian/Ubuntu package.
    Deb,
    /// The Fedora/openSUSE package.
    Rpm,
    Homebrew,
    Scoop,
    Winget,
    /// `rdownloader-bin` from the Arch User Repository.
    Aur,
    /// The container image.
    Docker,
    Unknown,
}

/// What the person does about an available update, by installation kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateAction {
    /// Download the release's artifact and replace the program by hand: a build from source,
    /// or a layout this build does not know.
    Download,
    /// Download, install and restart by itself, taking the switch back when the new version
    /// does not prove healthy (RD-180-02): the portable archive and the Windows installer.
    Install,
    /// Run a package manager's or a container runtime's command.
    Command {
        command: String,
        /// A stable code the interface translates: a second way, or what follows the command.
        hint: Option<&'static str>,
    },
}

impl InstallKind {
    /// The stable name used in the marker file, the API and the interface.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Msi => "msi",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Homebrew => "homebrew",
            Self::Scoop => "scoop",
            Self::Winget => "winget",
            Self::Aur => "aur",
            Self::Docker => "docker",
            Self::Unknown => "unknown",
        }
    }

    /// The kind `value` names; anything else is `None`, never a guess.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "portable" => Self::Portable,
            "msi" => Self::Msi,
            "deb" => Self::Deb,
            "rpm" => Self::Rpm,
            "homebrew" => Self::Homebrew,
            "scoop" => Self::Scoop,
            "winget" => Self::Winget,
            "aur" => Self::Aur,
            "docker" => Self::Docker,
            _ => return None,
        })
    }

    /// Whether this kind's channel carries pre-releases at all. The tap, the bucket, winget and
    /// the AUR follow the newest plain `vX.Y.Z` only (`release.yml`), so offering them a beta
    /// would show a command that installs nothing new.
    #[must_use]
    pub fn receives_betas(self) -> bool {
        !matches!(
            self,
            Self::Homebrew | Self::Scoop | Self::Winget | Self::Aur
        )
    }

    /// Whether this kind installs an update itself (RD-180-02).
    #[must_use]
    pub fn installs_itself(self) -> bool {
        matches!(self, Self::Portable | Self::Msi)
    }

    /// What the person does to install `version`.
    #[must_use]
    pub fn action(self, version: &str) -> UpdateAction {
        let command =
            |command: String, hint: Option<&'static str>| UpdateAction::Command { command, hint };
        match self {
            Self::Portable | Self::Msi => UpdateAction::Install,
            Self::Unknown => UpdateAction::Download,
            // The repository carries plain releases only (RD-180-10); a beta, or a package
            // installed from a download, is installed from the file, which the hint says.
            Self::Deb => command(
                "sudo apt update && sudo apt install --only-upgrade rdownloader".to_owned(),
                Some("update.hint.deb_file"),
            ),
            Self::Rpm => command(
                "sudo dnf upgrade --refresh rdownloader".to_owned(),
                Some("update.hint.rpm_file"),
            ),
            Self::Homebrew => command("brew upgrade rdownloader".to_owned(), None),
            Self::Scoop => command("scoop update rdownloader".to_owned(), None),
            Self::Winget => command("winget upgrade degoya.rDownloader".to_owned(), None),
            Self::Aur => command(
                "yay -S rdownloader-bin".to_owned(),
                Some("update.hint.aur_helper"),
            ),
            // `:latest` follows the newest plain release only; a beta is pulled by its tag.
            Self::Docker => {
                let tag = if crate::offer::parse_version(version)
                    .is_some_and(|version| !version.pre.is_empty())
                {
                    format!("v{}", version.trim_start_matches('v'))
                } else {
                    "latest".to_owned()
                };
                command(
                    format!("docker pull ghcr.io/degoya/rdownloader:{tag}"),
                    Some("update.hint.docker_recreate"),
                )
            }
        }
    }

    /// The running installation's kind.
    #[must_use]
    pub fn detect() -> Self {
        let executable = std::env::current_exe().ok().map(|path| canonical(&path));
        let named = std::env::var(INSTALL_KIND_ENV).ok();
        detect_from(executable.as_deref(), named.as_deref(), in_container())
    }
}

/// The canonical path where it resolves, so a symlink (`/opt/homebrew/bin/rdownloader`) leads
/// to the layout it points into; the path as given otherwise.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|canonical| strip_verbatim(&canonical))
        .unwrap_or_else(|_| path.to_owned())
}

/// Windows' canonical form is `\\?\C:\…`; the layout checks want the ordinary one.
fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path.to_owned(),
    }
}

/// Container detection, from the markers the runtimes leave behind (as `rd_files` does).
fn in_container() -> bool {
    cfg!(target_os = "linux")
        && (Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists())
}

/// [`InstallKind::detect`] from explicit inputs, so every rule is testable with a fixture.
#[must_use]
pub fn detect_from(
    executable: Option<&Path>,
    named: Option<&str>,
    in_container: bool,
) -> InstallKind {
    if let Some(kind) = named.and_then(InstallKind::parse) {
        return kind;
    }
    if in_container {
        return InstallKind::Docker;
    }
    let Some(executable) = executable else {
        return InstallKind::Unknown;
    };
    if let Some(kind) = kind_from_layout(executable) {
        return kind;
    }
    if let Some(kind) = marker_locations(executable)
        .iter()
        .find_map(|path| read_marker(path))
    {
        return kind;
    }
    // The archives carry no marker (the installers do, RD-180-05); they carry `VERSION.txt`
    // beside the executable, which a build from source does not.
    if executable
        .parent()
        .is_some_and(|directory| directory.join(VERSION_FILE).is_file())
    {
        return InstallKind::Portable;
    }
    InstallKind::Unknown
}

/// The package managers whose layout names them.
fn kind_from_layout(executable: &Path) -> Option<InstallKind> {
    let parts: Vec<String> = executable
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    let follows = |first: &str, second: &str| {
        parts
            .windows(2)
            .any(|pair| pair[0] == first && pair[1] == second)
    };
    // Scoop: `<root>\apps\rdownloader\<version or current>\rdownloader.exe`, with a root that
    // names Scoop (`~\scoop`, `C:\ProgramData\scoop`), so an archive a person unpacked into a
    // folder of their own called `apps` is not taken for it.
    let under_scoop = parts.iter().any(|part| part.contains("scoop"));
    if under_scoop
        && parts.windows(4).any(|window| {
            window[0] == "apps"
                && window[1] == "rdownloader"
                && window[3].starts_with("rdownloader")
                && window[3].ends_with(".exe")
        })
    {
        return Some(InstallKind::Scoop);
    }
    // Homebrew: `<prefix>/Cellar/rdownloader/<version>/…`, on macOS and on Linux.
    if follows("cellar", "rdownloader") {
        return Some(InstallKind::Homebrew);
    }
    // winget's portable installs: `%LOCALAPPDATA%\Microsoft\WinGet\Packages\<id>_<source>\`.
    if follows("winget", "packages") {
        return Some(InstallKind::Winget);
    }
    None
}

/// Where the marker is looked for: beside the executable, then, for an executable in a `bin`
/// directory, in `../lib/rdownloader/`.
fn marker_locations(executable: &Path) -> Vec<PathBuf> {
    let Some(directory) = executable.parent() else {
        return Vec::new();
    };
    let mut locations = vec![directory.join(INSTALL_KIND_FILE)];
    if directory.file_name().is_some_and(|name| name == "bin")
        && let Some(prefix) = directory.parent()
    {
        locations.push(
            prefix
                .join("lib")
                .join("rdownloader")
                .join(INSTALL_KIND_FILE),
        );
    }
    locations
}

/// The kind a marker file names; `None` when it is missing, unreadable or names nothing known.
fn read_marker(path: &Path) -> Option<InstallKind> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(MAX_MARKER_BYTES).read_to_string(&mut text).ok()?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    let kind = InstallKind::parse(line);
    if kind.is_none() {
        tracing::warn!(path = %path.display(), value = line, "the install-kind marker names no known kind");
    }
    kind
}

#[cfg(test)]
#[path = "install_kind_tests.rs"]
mod tests;
