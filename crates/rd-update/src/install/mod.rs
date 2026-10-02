//! Installing a verified update and taking it back (RD-180-02): the files of the switch, the
//! journal that makes it recoverable, and what a start does with an interrupted one.
//!
//! Only a portable archive and the Windows installer install themselves (owner, 2026-09-30);
//! every other kind shows its package manager's command ([`InstallKind::installs_itself`]).
//!
//! **Who does what.** The running service verifies and downloads the artifact
//! ([`crate::download_verified`]), writes the backup before the update (RD-180-03), writes the
//! [`Journal`] in phase [`Phase::Handed`] and starts the updater: a copy of its own executable
//! outside the program folder ([`process::launch_updater`]), because a running `.exe` cannot be
//! replaced on Windows. The updater (`rdownloader apply-update`) holds [`UpdaterLock`] for its
//! whole life, stops the service over the local control token, switches the files, starts the
//! new version and waits for its health to name the target version. A failure takes the switch
//! back, puts the database copy from before the update in place and starts the old version.
//!
//! **The journal** (`<data>/update/journal.json`) is written atomically before every step, so a
//! stop anywhere leaves a phase the next start can act on ([`recover`]): nothing changed yet —
//! the update is recorded as failed; the switch half done — it is taken back; the switch done but
//! never proven — the first start of the new version may prove it, a second one rolls back.
//! Whether the updater still runs is its lock, which the operating system releases with the
//! process, never a process id that may have been reused.
//!
//! **The portable switch** ([`portable`]) unpacks into `<install>/.update-<version>/`, moves each
//! top-level entry of the new archive that exists now into `<install>/.previous/`, then the new
//! one into its place. Only the archive's own entries move: `data/`, `downloads/`, `logs/` and
//! whatever else lives beside the program stays. `.previous/` is kept until the next start of
//! the proven version, then removed. Leftovers are never removed in place but moved into
//! `<install>/.trash/` first ([`trash`]): what a running program still holds stays there for a
//! later sweep instead of failing the next update.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::InstallKind;

#[cfg(test)]
pub(crate) mod fixture;
mod journal_check;
pub mod portable;
pub mod process;
pub mod recover;
pub mod steps;
pub mod trash;

/// Below the data directory: what an update keeps between the service and the updater.
pub const UPDATE_DIR: &str = "update";
/// Inside [`UPDATE_DIR`].
pub const JOURNAL_FILE: &str = "journal.json";
/// Inside [`UPDATE_DIR`]: held by the updater for as long as it runs.
pub const LOCK_FILE: &str = "updater.lock";
/// Inside [`UPDATE_DIR`]: the verified downloads.
pub const DOWNLOAD_DIR: &str = "download";
/// Inside [`UPDATE_DIR`]: the installer of the running version, kept for a rollback (MSI).
pub const INSTALLER_DIR: &str = "installer";
/// Inside [`UPDATE_DIR`]: a journal [`Journal::check`] refused, kept for a person to read.
pub const REJECTED_FILE: &str = "journal.rejected.json";
/// Below the data directory: the database copies the backup before an update writes
/// (`rd_backup::pre_update::DIRECTORY`).
pub const PRE_UPDATE_DIR: &str = "pre-update";
/// Beside the program files: the entries the switch replaced.
pub const PREVIOUS_DIR: &str = ".previous";
/// How long the updater waits for the new version to answer with its version.
pub const DEFAULT_HEALTH_TIMEOUT_SECS: u64 = 90;
/// The hidden subcommand the updater runs as.
pub const APPLY_COMMAND: &str = "apply-update";

/// `<data>/update`.
#[must_use]
pub fn update_dir(data: &Path) -> PathBuf {
    data.join(UPDATE_DIR)
}

/// Why an install was refused or failed: a stable code the interface translates, and what
/// happened in words for the log.
#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
pub struct InstallError {
    pub code: &'static str,
    pub detail: String,
}

impl InstallError {
    #[must_use]
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// What the service hands the updater. Every path is absolute.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Plan {
    pub kind: InstallKind,
    pub from_version: String,
    pub target_version: String,
    /// The verified download.
    pub artifact: PathBuf,
    pub sha256: String,
    pub size: u64,
    /// The folder of the running executable.
    pub install_dir: PathBuf,
    /// The executable's file name in it, `rdownloader` or `rdownloader.exe`.
    pub executable: String,
    pub data_dir: PathBuf,
    pub database: PathBuf,
    /// The checked copy of the database the backup before the update wrote; put back when the
    /// new version was started and did not prove healthy.
    #[serde(default)]
    pub database_copy: Option<PathBuf>,
    /// The process the updater stops first.
    pub service_pid: u32,
    /// The service's own arguments and folder, so the new version starts as the old one did.
    pub service_args: Vec<String>,
    pub service_cwd: PathBuf,
    pub health_timeout_secs: u64,
    /// MSI: the installer of the running version, when the last update kept it.
    #[serde(default)]
    pub previous_installer: Option<PathBuf>,
    /// Its SHA-256 as that update checked it (`steps::kept_installer_sha256`): a rollback
    /// installs it only while it still matches.
    #[serde(default)]
    pub previous_installer_sha256: Option<String>,
}

impl Plan {
    /// Refuses a plan the updater must not act on.
    ///
    /// # Errors
    ///
    /// `update.plan_invalid` with what is wrong.
    pub fn validate(&self) -> Result<(), InstallError> {
        let invalid = |detail: String| Err(InstallError::new("update.plan_invalid", detail));
        if !self.kind.installs_itself() {
            return invalid(format!("{} does not install itself", self.kind.as_str()));
        }
        for version in [&self.from_version, &self.target_version] {
            if !is_plain_version(version) {
                return invalid(format!("{version:?} is not a plain version"));
            }
        }
        if self.executable.is_empty()
            || self.executable.contains(['/', '\\'])
            || self.executable.starts_with('.')
        {
            return invalid(format!("{:?} is not a file name", self.executable));
        }
        if let Some(digest) = &self.previous_installer_sha256
            && !steps::is_sha256(digest)
        {
            return invalid(format!("{digest:?} is not a SHA-256"));
        }
        for path in [
            &self.artifact,
            &self.install_dir,
            &self.data_dir,
            &self.database,
            &self.service_cwd,
        ] {
            if !path.is_absolute() {
                return invalid(format!("{} is not absolute", path.display()));
            }
        }
        Ok(())
    }
}

/// Letters, digits, `.`, `_`, `+` and `-`, at most 64 of them: safe in a folder name.
#[must_use]
pub fn is_plain_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && !version.starts_with('.')
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

/// Where an update stands.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The service wrote the plan and started the updater; nothing has changed.
    Handed,
    /// The updater asked the service to stop.
    Stopping,
    /// The new files are unpacked beside the program; nothing live has changed.
    Staged,
    /// Entries are being moved (portable) or the installer runs (MSI).
    Switching,
    /// The new version is in place and not yet proven.
    Switched,
    /// The new version answered with its version: done.
    Verified,
    /// The switch is being taken back.
    RollingBack,
    /// The previous version is back; `reason` says why.
    RolledBack,
    /// Nothing was switched, or the switch could not be taken back; `reason` says which.
    Failed,
}

impl Phase {
    /// The stable name in the journal and the API.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Handed => "handed",
            Self::Stopping => "stopping",
            Self::Staged => "staged",
            Self::Switching => "switching",
            Self::Switched => "switched",
            Self::Verified => "verified",
            Self::RollingBack => "rolling_back",
            Self::RolledBack => "rolled_back",
            Self::Failed => "failed",
        }
    }

    /// Whether the update has ended, one way or the other.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Verified | Self::RolledBack | Self::Failed)
    }
}

/// The record of one update, from the service's hand-over to its end.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Journal {
    pub plan: Plan,
    pub phase: Phase,
    /// Portable: the top-level entries of the new archive, the ones the switch moves.
    #[serde(default)]
    pub entries: Vec<String>,
    /// Portable: those of [`Self::entries`] that existed before and went to `.previous`.
    #[serde(default)]
    pub replaced: Vec<String>,
    /// Whether the new version was started, and with it may have changed the database.
    #[serde(default)]
    pub new_started: bool,
    /// Starts of the new version after the updater was gone (see [`recover`]).
    #[serde(default)]
    pub start_attempts: u32,
    /// The stable code of why it failed or was rolled back.
    #[serde(default)]
    pub reason: Option<String>,
    /// The same in words, for the log.
    #[serde(default)]
    pub detail: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Whether the leftovers of a finished update were removed.
    #[serde(default)]
    pub cleaned: bool,
}

impl Journal {
    /// A journal for `plan` in phase [`Phase::Handed`], not yet written.
    #[must_use]
    pub fn begin(plan: Plan) -> Self {
        let now = Utc::now();
        Self {
            plan,
            phase: Phase::Handed,
            entries: Vec::new(),
            replaced: Vec::new(),
            new_started: false,
            start_attempts: 0,
            reason: None,
            detail: None,
            started_at: now,
            updated_at: now,
            cleaned: false,
        }
    }

    /// `<data>/update/journal.json`.
    #[must_use]
    pub fn path(data: &Path) -> PathBuf {
        update_dir(data).join(JOURNAL_FILE)
    }

    /// The journal of the data directory, if an update was ever started from it.
    ///
    /// A journal [`Self::check`] refuses is acted on by nobody: it is logged, moved aside to
    /// [`REJECTED_FILE`] and read as none (security review 2026-09-30, finding 5).
    ///
    /// # Errors
    ///
    /// When it exists and cannot be read or parsed.
    pub fn read(data: &Path) -> Result<Option<Self>> {
        let path = Self::path(data);
        let journal: Self = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("read {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        if let Err(error) = journal.check(data) {
            let aside = update_dir(data).join(REJECTED_FILE);
            tracing::error!(code = error.code, detail = %error.detail, aside = %aside.display(), "the update journal is refused and nothing acts on it");
            rename(&path, &aside)?;
            return Ok(None);
        }
        Ok(Some(journal))
    }

    /// Writes the journal where the plan's data directory keeps it, atomically.
    ///
    /// # Errors
    ///
    /// When it cannot be written; nothing may act on a step the journal does not record.
    pub fn write(&mut self) -> Result<()> {
        self.updated_at = Utc::now();
        let path = Self::path(&self.plan.data_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        write_atomically(&path, &serde_json::to_vec_pretty(self)?)
    }

    /// Moves to `phase` and writes.
    ///
    /// # Errors
    ///
    /// As [`Self::write`].
    pub fn advance(&mut self, phase: Phase) -> Result<()> {
        self.phase = phase;
        self.write()
    }

    /// Ends the update in `phase` with `code` and writes.
    ///
    /// # Errors
    ///
    /// As [`Self::write`].
    pub fn end(&mut self, phase: Phase, code: &str, detail: impl Into<String>) -> Result<()> {
        self.phase = phase;
        self.reason = Some(code.to_owned());
        self.detail = Some(detail.into());
        self.write()
    }

    /// `<install>/.update-<version>`: where the new files are unpacked.
    #[must_use]
    pub fn staged_dir(&self) -> PathBuf {
        self.plan
            .install_dir
            .join(format!(".update-{}", self.plan.target_version))
    }

    /// `<install>/.previous`.
    #[must_use]
    pub fn previous_dir(&self) -> PathBuf {
        self.plan.install_dir.join(PREVIOUS_DIR)
    }

    /// `<install>/.failed-<version>`: a rolled-back version's files, until they can be removed.
    #[must_use]
    pub fn failed_dir(&self) -> PathBuf {
        self.plan
            .install_dir
            .join(format!(".failed-{}", self.plan.target_version))
    }

    /// The executable in the program folder.
    #[must_use]
    pub fn executable(&self) -> PathBuf {
        self.plan.install_dir.join(&self.plan.executable)
    }
}

/// The updater's lock on `<data>/update/updater.lock`, released with the process.
#[derive(Debug)]
pub struct UpdaterLock {
    _file: fs::File,
}

impl UpdaterLock {
    /// Takes the lock; `None` while another updater holds it.
    ///
    /// # Errors
    ///
    /// When the lock file cannot be opened.
    pub fn acquire(data: &Path) -> Result<Option<Self>> {
        let directory = update_dir(data);
        fs::create_dir_all(&directory)
            .with_context(|| format!("create {}", directory.display()))?;
        let path = directory.join(LOCK_FILE);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => {
                Err(error).with_context(|| format!("lock {}", path.display()))
            }
        }
    }
}

/// Whether an updater runs for this data directory right now.
#[must_use]
pub fn updater_running(data: &Path) -> bool {
    if !update_dir(data).join(LOCK_FILE).exists() {
        return false;
    }
    matches!(UpdaterLock::acquire(data), Ok(None))
}

/// Refuses an install that cannot succeed: a program folder this process may not write, or a
/// volume without twice the artifact's size free (the unpacked copy and the previous one).
///
/// # Errors
///
/// `update.install_dir_not_writable` or `update.not_enough_space`.
pub fn preflight(install_dir: &Path, artifact_size: u64) -> Result<(), InstallError> {
    let probe = install_dir.join(".update-probe");
    let written = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
        .and_then(|mut file| file.write_all(b"probe"));
    let _ = fs::remove_file(&probe);
    if let Err(error) = written {
        return Err(InstallError::new(
            "update.install_dir_not_writable",
            format!("{} cannot be written: {error}", install_dir.display()),
        ));
    }
    let required = artifact_size.saturating_mul(2);
    match fs4::available_space(install_dir) {
        Ok(free) if free < required => Err(InstallError::new(
            "update.not_enough_space",
            format!(
                "{} has {free} bytes free, the update needs {required}",
                install_dir.display()
            ),
        )),
        Ok(_) => Ok(()),
        // A volume that does not say how much is free is not refused for it.
        Err(error) => {
            tracing::warn!(%error, "the free space of the program folder is unknown");
            Ok(())
        }
    }
}

pub(crate) fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

pub(crate) fn remove_any(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .with_context(|| format!("remove {}", path.display()))
}

/// How long a rename waits for a handle that is still being closed (Windows only).
const RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Renames `from` to `to`, retrying a sharing or access violation on Windows: a process that
/// just ended, or a virus scanner looking at a new executable, holds its files a moment longer.
pub(crate) fn rename(from: &Path, to: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    loop {
        match fs::rename(from, to) {
            Err(error)
                if cfg!(windows)
                    && matches!(error.raw_os_error(), Some(5 | 32))
                    && started.elapsed() < RELEASE_WAIT =>
            {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            result => {
                return result
                    .with_context(|| format!("move {} to {}", from.display(), to.display()));
            }
        }
    }
}

/// Makes a rename in `directory` durable. On Windows a directory cannot be opened this way.
pub(crate) fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    let _ = fs::File::open(directory).and_then(|handle| handle.sync_all());
    #[cfg(not(unix))]
    let _ = directory;
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        sync_directory(parent);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
