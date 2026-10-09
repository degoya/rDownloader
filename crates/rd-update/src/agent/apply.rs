//! Installing the capture agent's own update (RD-1210-03), with the journal, the portable switch
//! and the recovery of the service's (RD-180-02).
//!
//! **Who does what.** The running agent — or `rdownloader-capture update` — verifies and
//! downloads its archive ([`crate::download_verified`]), writes the journal `handed` in
//! `<agent configuration>/update/journal.json` ([`plan`], [`hand_over`]) and starts a copy of
//! itself outside the program folder as `apply-update` (`process::launch_agent_updater`). The
//! updater holds the update lock for its whole life and runs [`apply`]: checks the archive and the
//! folder once more, stages and switches the files, and asks whether the new version started.
//!
//! **The restart** is `rd_capture`'s relaunch rule (RD-190-07): a running agent sees its program
//! file replaced and continues as the new one, with its own arguments. Once the new agent runs it
//! writes its version into [`PROOF_FILE`] ([`started`]); the updater waits for that proof
//! ([`await_proof`], [`START_TIMEOUT_SECS`]) and otherwise takes the switch back — which the
//! running program sees the same way, and so continues as the old one. With no agent running, the
//! new program only has to name its version ([`answers_version`]).
//!
//! **No database.** The plan's `database` is a name nothing writes ([`NO_DATABASE`]) and it carries
//! no database copy, so the roll-back puts back the program files only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::install::{
    self, InstallError, Journal, Phase, Plan, portable, process, recover, remove_any, steps,
    update_dir,
};
use crate::{Artifact, InstallKind, is_newer};

/// Inside `<data>/update`: the version the agent that started last runs, while an update is
/// recorded.
pub const PROOF_FILE: &str = "agent-started";
/// The plan's database: the agent keeps none, and nothing ever writes this name.
pub const NO_DATABASE: &str = "no-database";
/// How long the updater waits for the new agent's proof: the relaunch rule notices the replaced
/// file within half a minute, and the new agent waits up to ten seconds for the old one's lock.
pub const START_TIMEOUT_SECS: u64 = 120;
/// How long the new program may take to name its version when no agent runs.
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a write of the proof waits for a handle that is still being closed (Windows).
const RELEASE_WAIT: Duration = Duration::from_secs(5);

/// What an agent hands its updater: the update of the agent at `executable` from `from` to
/// `to`, from the verified `download` of `artifact`, with its journal in `data`. `args` are the
/// arguments the running agent was started with, for a restart of the old version.
///
/// # Errors
///
/// `update.not_newer` for a version that is not newer than the running one — never a downgrade,
/// whatever asked for it — and `update.plan_invalid` for a plan the updater would refuse.
pub fn plan(
    executable: &Path,
    data: &Path,
    (from, to): (&str, &str),
    download: &Path,
    artifact: &Artifact,
    args: Vec<String>,
) -> Result<Plan, InstallError> {
    if !is_newer(to, from) {
        return Err(InstallError::new(
            "update.not_newer",
            format!("{to} is not newer than the running {from}"),
        ));
    }
    let invalid = |detail: String| InstallError::new("update.plan_invalid", detail);
    let install_dir = executable
        .parent()
        .ok_or_else(|| invalid(format!("{} has no folder", executable.display())))?
        .to_path_buf();
    let name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid(format!("{} has no file name", executable.display())))?
        .to_owned();
    let plan = Plan {
        kind: InstallKind::Portable,
        from_version: from.to_owned(),
        target_version: to.to_owned(),
        artifact: download.to_path_buf(),
        sha256: artifact.sha256.clone(),
        size: artifact.size,
        install_dir: install_dir.clone(),
        executable: name,
        data_dir: data.to_path_buf(),
        database: data.join(NO_DATABASE),
        database_copy: None,
        service_pid: std::process::id(),
        service_args: args,
        service_cwd: install_dir,
        health_timeout_secs: START_TIMEOUT_SECS,
        previous_installer: None,
        previous_installer_sha256: None,
    };
    plan.validate()?;
    Ok(plan)
}

/// Writes the journal `handed` and starts the updater. Refused while an update of this agent is
/// under way.
///
/// # Errors
///
/// `update.in_progress` while an updater runs or a journal records an unfinished update;
/// `update.start_failed` when the updater cannot be started, which the journal then records.
pub fn hand_over(plan: Plan) -> Result<Journal, InstallError> {
    let data = plan.data_dir.clone();
    let busy = install::updater_running(&data)
        || Journal::read(&data)
            .ok()
            .flatten()
            .is_some_and(|journal| !journal.phase.is_terminal());
    if busy {
        return Err(InstallError::new(
            "update.in_progress",
            "an update of this agent is under way",
        ));
    }
    clear_proof(&data);
    let mut journal = Journal::begin(plan);
    let failed =
        |error: anyhow::Error| InstallError::new("update.start_failed", format!("{error:#}"));
    journal.write().map_err(failed)?;
    if let Err(error) = process::launch_agent_updater(&journal) {
        let detail = format!("{error:#}");
        let _ = journal.end(Phase::Failed, "update.start_failed", detail.clone());
        return Err(InstallError::new("update.start_failed", detail));
    }
    Ok(journal)
}

/// The updater's work after the hand-over: checks the archive and the folder, stages and switches,
/// then asks `started` whether the new version started; takes the switch back when it did not.
/// Starts and stops no program — `started` waits, the caller starts what has to run.
///
/// # Errors
///
/// When the journal cannot be written; every other outcome is the journal's phase.
pub fn apply(
    journal: &mut Journal,
    started: &mut dyn FnMut(&Journal) -> Result<(), InstallError>,
) -> Result<()> {
    if let Err(error) = journal.plan.validate() {
        return journal.end(Phase::Failed, error.code, error.detail);
    }
    let fit = steps::verify_artifact(&journal.plan)
        .and_then(|()| install::preflight(&journal.plan.install_dir, journal.plan.size));
    if let Err(error) = fit {
        return journal.end(Phase::Failed, error.code, error.detail);
    }
    if let Err(error) = portable::stage(journal) {
        let _ = remove_any(&journal.staged_dir());
        let code = error
            .downcast_ref::<InstallError>()
            .map_or("update.unpack_failed", |error| error.code);
        return journal.end(Phase::Failed, code, format!("{error:#}"));
    }
    clear_proof(&journal.plan.data_dir);
    if let Err(error) = portable::switch(journal) {
        return take_back(journal, "update.switch_failed", format!("{error:#}"));
    }
    journal.new_started = true;
    journal.write()?;
    match started(journal) {
        Ok(()) => journal.advance(Phase::Verified),
        Err(error) => take_back(journal, error.code, error.detail),
    }
}

/// Takes the switch back and ends the journal with `code`.
fn take_back(journal: &mut Journal, code: &str, detail: String) -> Result<()> {
    tracing::error!(code, %detail, "the agent's new version is taken back");
    journal.reason = Some(code.to_owned());
    journal.detail = Some(detail.clone());
    journal.advance(Phase::RollingBack)?;
    match portable::roll_back(journal) {
        Ok(()) => journal.end(Phase::RolledBack, code, detail),
        Err(error) => journal.end(
            Phase::Failed,
            "update.rollback_failed",
            format!("{detail}; taking it back failed: {error:#}"),
        ),
    }
}

/// `<data>/update/agent-started`.
#[must_use]
pub fn proof_path(data: &Path) -> PathBuf {
    update_dir(data).join(PROOF_FILE)
}

/// The version the proof names, if there is one.
#[must_use]
pub fn read_proof(data: &Path) -> Option<String> {
    let text = fs::read_to_string(proof_path(data)).ok()?;
    let version = text.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// Removes the proof, so only a start after this moment counts.
pub fn clear_proof(data: &Path) {
    if let Err(error) = remove_any(&proof_path(data)) {
        tracing::warn!(%error, "the agent's start proof could not be removed");
    }
}

/// Waits until the proof names `version`, polling every `poll`; `alive` says whether the program
/// that should write it may still do so.
///
/// # Errors
///
/// `update.new_version_exited` once `alive` says no, `update.health_timeout` after `timeout`.
pub fn await_proof(
    data: &Path,
    version: &str,
    (timeout, poll): (Duration, Duration),
    alive: &mut dyn FnMut() -> bool,
) -> Result<(), InstallError> {
    let deadline = Instant::now() + timeout;
    loop {
        if read_proof(data).as_deref() == Some(version) {
            return Ok(());
        }
        if !alive() {
            return Err(InstallError::new(
                "update.new_version_exited",
                format!("{version} ended before it started"),
            ));
        }
        if Instant::now() >= deadline {
            return Err(InstallError::new(
                "update.health_timeout",
                format!(
                    "{version} did not start within {} seconds",
                    timeout.as_secs()
                ),
            ));
        }
        std::thread::sleep(poll);
    }
}

/// Runs `executable --version` and checks it names `version`: the proof when no agent runs.
///
/// # Errors
///
/// `update.new_version_exited` when it cannot be started, ends with a failure or names another
/// version; `update.health_timeout` when it does not end in time.
pub fn answers_version(executable: &Path, version: &str) -> Result<(), InstallError> {
    use rd_files::NoConsoleWindow as _;
    let failed = |detail: String| InstallError::new("update.new_version_exited", detail);
    let mut child = Command::new(executable)
        .no_console_window()
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| failed(format!("start {}: {error}", executable.display())))?;
    let deadline = Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(InstallError::new(
                    "update.health_timeout",
                    format!("{} --version did not end", executable.display()),
                ));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| failed(format!("read {}: {error}", executable.display())))?;
    let named = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .any(|word| word == version);
    if output.status.success() && named {
        Ok(())
    } else {
        Err(failed(format!(
            "{} --version does not name {version}",
            executable.display()
        )))
    }
}

/// What the agent does with a recorded update when it starts, before anything else: as the
/// service does (`recover::recover_at_start`).
///
/// # Errors
///
/// As `recover::recover_at_start`.
pub fn at_start(
    data: &Path,
    executable: &Path,
    running_version: &str,
) -> Result<recover::Recovery> {
    recover::recover_at_start(data, executable, running_version)
}

/// The agent runs: while an update is recorded, it writes its version as the proof the updater
/// waits for, and proves an update whose updater is gone (`recover::confirm_started`). Returns
/// whether this start proved one.
///
/// # Errors
///
/// When the journal or the proof cannot be read or written.
pub fn started(data: &Path, running_version: &str) -> Result<bool> {
    if !Journal::path(data).is_file() {
        return Ok(false);
    }
    rd_files::durable::write_atomically(
        &proof_path(data),
        running_version.as_bytes(),
        RELEASE_WAIT,
    )?;
    recover::confirm_started(data, running_version)
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
