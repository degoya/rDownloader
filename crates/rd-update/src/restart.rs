//! Restarting the running version (RD-1240-32): how this installation comes back after a stop,
//! and the plan the relauncher works from.
//!
//! A plugin installed or updated runs only from the next start; so do a few other choices. The
//! service can restart itself in one of three ways, by how it was started ([`decide_how`]):
//!
//! * **supervisor** -- a container (`InstallKind::Docker`), or systemd ([`SYSTEMD_ENV`] in the
//!   environment, which systemd sets for every unit it starts): the service stops as for any stop
//!   and ends with [`RESTART_EXIT_CODE`] instead of `0`. The packaged unit's `Restart=on-failure`
//!   and the compose file's `restart: unless-stopped` start it again. A container started without
//!   a restart policy stays stopped; the interface says so.
//! * **self** -- every other installation whose executable is known and whose arguments are text:
//!   the relauncher, `rdownloader restart-service`, started like the updater from a copy of the
//!   executable in `<data>/update/updater/` ([`launch_relauncher`]), stops the service over the
//!   local control token, starts the same executable with the same arguments in the same folder
//!   (`install::process::start_program`), and waits until its health route answers with the same
//!   version -- the updater's steps without the switch, and no second mechanism beside them.
//! * **manual** -- neither: the service stops with [`RESTART_EXIT_CODE`], and whoever started it
//!   starts it again.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::InstallKind;
use crate::install::{InstallError, is_plain_version, process, update_dir};

/// What the service ends with when a supervisor is to start it again: `EX_TEMPFAIL` of
/// `sysexits.h`, "try again later". Not `0`, which `Restart=on-failure` reads as a wanted stop,
/// and none of the codes the service ends with otherwise (`1` for an error or a stop past its
/// deadline).
pub const RESTART_EXIT_CODE: i32 = 75;
/// The hidden subcommand the relauncher runs as.
pub const RESTART_COMMAND: &str = "restart-service";
/// Inside `<data>/update`: the plan the relauncher reads.
pub const PLAN_FILE: &str = "restart.json";
/// What systemd sets for every unit it starts (`systemd.exec(5)`, `$INVOCATION_ID`).
pub const SYSTEMD_ENV: &str = "INVOCATION_ID";
/// How long the relauncher waits for the restarted service to answer.
pub const DEFAULT_HEALTH_TIMEOUT_SECS: u64 = 90;

/// How the service comes back after a restart.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartHow {
    /// The relauncher starts the same version.
    #[serde(rename = "self")]
    Relaunch,
    /// systemd or the container runtime starts it on [`RESTART_EXIT_CODE`].
    Supervisor,
    /// It stops; whoever started it starts it again.
    Manual,
}

impl RestartHow {
    /// The stable name in the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relaunch => "self",
            Self::Supervisor => "supervisor",
            Self::Manual => "manual",
        }
    }
}

/// Who starts the service again for [`RestartHow::Supervisor`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Supervisor {
    Systemd,
    /// The container runtime, by the container's restart policy.
    Container,
}

impl Supervisor {
    /// The stable name in the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Container => "container",
        }
    }
}

/// What decides the way, read once at the start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Environment {
    /// systemd started this process ([`SYSTEMD_ENV`]).
    pub systemd: bool,
    /// The executable is known and every argument is text, so the same start can be repeated.
    pub relaunchable: bool,
}

impl Environment {
    /// This process's environment.
    #[must_use]
    pub fn detect() -> Self {
        Self {
            systemd: systemd_from(|name| std::env::var_os(name)),
            relaunchable: std::env::current_exe().is_ok()
                && std::env::args_os()
                    .skip(1)
                    .all(|argument| argument.to_str().is_some()),
        }
    }
}

/// Whether the environment `lookup` reads names systemd as the starter.
#[must_use]
pub fn systemd_from(lookup: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    lookup(SYSTEMD_ENV).is_some_and(|value| !value.is_empty())
}

/// The way an installation of `kind` in `environment` restarts; see the module documentation.
/// A container wins over systemd: inside one, the runtime is what starts the process again.
#[must_use]
pub fn decide_how(kind: InstallKind, environment: Environment) -> (RestartHow, Option<Supervisor>) {
    if kind == InstallKind::Docker {
        (RestartHow::Supervisor, Some(Supervisor::Container))
    } else if environment.systemd {
        (RestartHow::Supervisor, Some(Supervisor::Systemd))
    } else if environment.relaunchable {
        (RestartHow::Relaunch, None)
    } else {
        (RestartHow::Manual, None)
    }
}

/// What the service hands the relauncher. Every path is absolute.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RestartPlan {
    /// The running version, which the restarted service has to answer as.
    pub version: String,
    /// The running executable.
    pub executable: PathBuf,
    pub data_dir: PathBuf,
    /// The process the relauncher stops.
    pub service_pid: u32,
    /// The service's own arguments and folder, so it starts as it was started.
    pub service_args: Vec<String>,
    pub service_cwd: PathBuf,
    pub health_timeout_secs: u64,
    pub requested_at: DateTime<Utc>,
}

impl RestartPlan {
    /// `<data>/update/restart.json`.
    #[must_use]
    pub fn path(data: &Path) -> PathBuf {
        update_dir(data).join(PLAN_FILE)
    }

    /// The executable's file name, the image the relauncher ends by force when it must.
    #[must_use]
    pub fn image(&self) -> String {
        self.executable
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Refuses a plan the relauncher must not act on.
    ///
    /// # Errors
    ///
    /// `restart.plan_invalid` with what is wrong.
    pub fn validate(&self) -> Result<(), InstallError> {
        let invalid = |detail: String| Err(InstallError::new("restart.plan_invalid", detail));
        if !is_plain_version(&self.version) {
            return invalid(format!("{:?} is not a plain version", self.version));
        }
        if self.image().is_empty() || self.image().starts_with('.') {
            return invalid(format!("{} names no program", self.executable.display()));
        }
        for path in [&self.executable, &self.data_dir, &self.service_cwd] {
            if !path.is_absolute() {
                return invalid(format!("{} is not absolute", path.display()));
            }
        }
        Ok(())
    }

    /// Writes the plan to [`Self::path`].
    ///
    /// # Errors
    ///
    /// When it cannot be written.
    pub fn write(&self) -> Result<PathBuf> {
        let path = Self::path(&self.data_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write {}", path.display()))?;
        Ok(path)
    }

    /// Reads the plan at `path`.
    ///
    /// # Errors
    ///
    /// When it is missing or not a plan.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("read {}", path.display()))
    }
}

/// Writes `plan` and starts the relauncher for it: a copy of the running executable in
/// `<data>/update/updater/`, as `restart-service --plan <plan>`, detached, so it outlives the
/// service it stops (`install::process::launch_copy_with`).
///
/// # Errors
///
/// When the plan is invalid, or it, the copy or the start fails.
pub fn launch_relauncher(plan: &RestartPlan) -> Result<()> {
    plan.validate()?;
    let path = plan.write()?;
    let args = [
        OsStr::new(RESTART_COMMAND),
        OsStr::new("--plan"),
        path.as_os_str(),
    ];
    process::launch_copy_with(
        &plan.data_dir,
        &plan.service_cwd,
        process::updater_file_name(),
        &args,
    )
}

#[cfg(test)]
#[path = "restart_tests.rs"]
mod tests;
