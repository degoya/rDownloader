//! The capture agent's own update (RD-1210-03): an agent installed without the service — the
//! service on a NAS, in Docker or on another machine — finds a newer version itself, says so in
//! its tray and installs it on request.
//!
//! * [`AgentSetup`] — whether the agent updates itself at all. With the service's executable in
//!   its folder it does not: the service's update replaces both (RD-180-02), and two updaters on
//!   one folder is exactly what this rule keeps out. Alone, the portable archive installs itself,
//!   a package manager's install shows its command, anything else the download.
//! * [`report`] — what the agent tells the service about its update, and the channel the service
//!   tells the agent: one header each way on the agent's settings poll.
//! * [`apply`] — the install: the portable switch of [`crate::install::portable`] with its journal
//!   in the agent's configuration directory, the new version's proof that it started, and the
//!   roll-back when that proof does not come in time.
//!
//! The check is the service's: [`crate::check`] over the same signed manifest and update key, and
//! [`crate::newest_agent_offer`], which never offers a version that is not newer than the running
//! one. The agent's archives are the manifest's `agent_artifacts`.

use std::path::{Component, Path};

use crate::{InstallKind, UpdateAction, install_kind, manifest::Channel};

pub mod apply;
pub mod report;

/// The agent's executable, as every release names it.
#[must_use]
pub fn agent_executable() -> &'static str {
    if cfg!(windows) {
        "rdownloader-capture.exe"
    } else {
        "rdownloader-capture"
    }
}

/// The service's executable, as every release names it.
#[must_use]
pub fn service_executable() -> &'static str {
    if cfg!(windows) {
        "rdownloader.exe"
    } else {
        "rdownloader"
    }
}

/// Whether the service's executable stands in `folder`: the archive, the installer and the
/// distribution packages all put both programs side by side.
#[must_use]
pub fn service_beside(folder: &Path) -> bool {
    folder.join(service_executable()).is_file()
}

/// How the running agent is installed, as far as its own update is concerned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentSetup {
    /// The service sits in the same folder and updates both; the agent offers nothing itself.
    WithService,
    /// Installed without the service, as this kind.
    Alone(InstallKind),
}

impl AgentSetup {
    /// The running agent's setup.
    #[must_use]
    pub fn detect() -> Self {
        let executable = std::env::current_exe()
            .ok()
            .map(|path| install_kind::canonical(&path));
        Self::of(executable.as_deref())
    }

    /// [`Self::detect`] for an explicit executable, so every rule is testable with a fixture.
    /// `RDOWNLOADER_INSTALL_KIND` is the service's and is not read here.
    #[must_use]
    pub fn of(executable: Option<&Path>) -> Self {
        if executable
            .and_then(Path::parent)
            .is_some_and(service_beside)
        {
            return Self::WithService;
        }
        if executable.is_some_and(in_homebrew_cellar) {
            return Self::Alone(InstallKind::Homebrew);
        }
        Self::Alone(install_kind::detect_from(executable, None, false))
    }

    /// The stable name in the report and the log: `with_service` or the install kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WithService => "with_service",
            Self::Alone(kind) => kind.as_str(),
        }
    }

    /// What the person does about `version`; `None` beside the service, which does it.
    ///
    /// Only the portable archive installs itself: the agent ships no installer of its own, and
    /// a package manager's install is that manager's to change.
    #[must_use]
    pub fn action(self, version: &str) -> Option<UpdateAction> {
        let Self::Alone(kind) = self else {
            return None;
        };
        Some(match kind {
            InstallKind::Portable => UpdateAction::Install,
            InstallKind::Homebrew => UpdateAction::Command {
                command: "brew upgrade rdownloader-capture".to_owned(),
                hint: None,
            },
            InstallKind::Msi | InstallKind::Docker | InstallKind::Unknown => UpdateAction::Download,
            other => other.action(version),
        })
    }

    /// The channel the agent reads: the connected service's when it said one, stable otherwise,
    /// and stable for a package manager that publishes no pre-releases.
    #[must_use]
    pub fn channel(self, service: Option<Channel>) -> Channel {
        match self {
            Self::Alone(kind) if !kind.receives_betas() => Channel::Stable,
            _ => service.unwrap_or(Channel::Stable),
        }
    }
}

/// Homebrew's own formula for the agent: `<prefix>/Cellar/rdownloader-capture/<version>/…`.
fn in_homebrew_cellar(executable: &Path) -> bool {
    let parts: Vec<String> = executable
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    parts
        .windows(2)
        .any(|pair| pair[0] == "cellar" && pair[1] == "rdownloader-capture")
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
