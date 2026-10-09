//! The agent's updater (RD-1210-03): `rdownloader-capture apply-update`, run from a copy of the
//! agent outside its folder, and what every start of the agent does with a recorded update.
//!
//! The updater holds the update lock for its whole life and runs `rd_update::agent::apply`: the
//! archive and the folder checked once more, the files staged and switched. Then it waits for
//! the new version: a running agent sees its program file replaced and continues as the new one
//! (`relaunch.rs`), which writes its version as the proof; with no agent running, the new program
//! only has to name its version. Without that in time, the switch is taken back — the running
//! program continues as the old one the same way — and an agent that ended with the new version is
//! started again as the old one.
//!
//! Exit codes, as the service's updater: `0` the new version runs, `2` the previous one again,
//! `1` anything else; the journal says what happened.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use rd_update::agent::apply;
use rd_update::install::recover::Recovery;
use rd_update::install::{InstallError, Journal, Phase, UpdaterLock, process, update_dir};

use crate::{config, instance};

/// How often the proof is looked for.
const PROOF_POLL: Duration = Duration::from_millis(500);
/// How long a roll-back waits for the running program to continue as the old version.
const BACK_WAIT: Duration = Duration::from_secs(45);
/// Inside `<configuration>/update`: the output of an agent the updater started again.
const AGENT_LOG: &str = "agent.log";

#[derive(Args)]
pub(crate) struct ApplyArgs {
    /// The journal the agent wrote: `<configuration>/update/journal.json`.
    #[arg(long)]
    journal: PathBuf,
}

pub(crate) fn apply_update(args: ApplyArgs) -> Result<()> {
    let data = args
        .journal
        .parent()
        .and_then(Path::parent)
        .context("the journal is not inside <configuration>/update")?
        .to_path_buf();
    let Some(lock) = UpdaterLock::acquire(&data)? else {
        anyhow::bail!("another updater runs for {}", data.display());
    };
    let mut journal =
        Journal::read(&data)?.context("no update is waiting in this configuration directory")?;
    anyhow::ensure!(
        journal.phase == Phase::Handed,
        "the update is {} already; only a handed-over one is applied",
        journal.phase.as_str()
    );
    let agent_was_running = agent_runs(&data);
    let target = journal.plan.target_version.clone();
    apply::apply(
        &mut journal,
        &mut |journal: &Journal| -> Result<(), InstallError> {
            if agent_was_running {
                let wait = Duration::from_secs(journal.plan.health_timeout_secs);
                apply::await_proof(
                    &journal.plan.data_dir,
                    &target,
                    (wait, PROOF_POLL),
                    &mut || true,
                )
            } else {
                apply::answers_version(&journal.executable(), &target)
            }
        },
    )?;
    if journal.phase == Phase::RolledBack && agent_was_running {
        continue_as_old(&journal);
    }
    tracing::info!(
        phase = journal.phase.as_str(),
        reason = journal.reason.as_deref().unwrap_or(""),
        "the agent's updater ends"
    );
    drop(lock);
    std::process::exit(match journal.phase {
        Phase::Verified => 0,
        Phase::RolledBack => 2,
        _ => 1,
    });
}

/// Whether an agent of this account runs: it holds the single-instance lock.
fn agent_runs(data: &Path) -> bool {
    matches!(
        instance::acquire(data, Duration::ZERO),
        Err(error) if error.is::<instance::AlreadyRunning>()
    )
}

/// After a roll-back: the program that runs continues as the old one by itself, which its proof
/// says; one that ended with the new version is started again as the old one.
fn continue_as_old(journal: &Journal) {
    let data = &journal.plan.data_dir;
    let from = &journal.plan.from_version;
    apply::clear_proof(data);
    if apply::await_proof(data, from, (BACK_WAIT, PROOF_POLL), &mut || true).is_ok()
        || agent_runs(data)
    {
        return;
    }
    let log = update_dir(data).join(AGENT_LOG);
    match process::spawn_detached(
        &journal.executable(),
        journal.plan.service_args.as_slice(),
        &journal.plan.service_cwd,
        &log,
        &log,
    ) {
        Ok(_) => tracing::info!(version = %from, "the previous agent is started again"),
        Err(error) => {
            tracing::error!(%error, "the previous agent could not be started; start it by hand")
        }
    }
}

/// What a start does with a recorded update, before anything else; `true` when the previous
/// version was put back and started in this one's place, so this process ends.
pub(crate) fn recover_at_start() -> bool {
    let (Ok(directory), Ok(executable)) = (config::config_directory(), std::env::current_exe())
    else {
        return false;
    };
    match apply::at_start(&directory, &executable, env!("CARGO_PKG_VERSION")) {
        Ok(Recovery::Continue) => false,
        Ok(Recovery::Restart(program)) => {
            tracing::warn!(program = %program.display(), "the previous agent is back; it starts in this one's place");
            let mut command = std::process::Command::new(&program);
            command
                .args(std::env::args_os().skip(1))
                .stdin(std::process::Stdio::null());
            rd_files::NoConsoleWindow::no_console_window(&mut command);
            match command.spawn() {
                Ok(_) => true,
                Err(error) => {
                    tracing::error!(%error, "the previous agent could not be started; this one runs on");
                    false
                }
            }
        }
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "the agent's interrupted update could not be ended"
            );
            false
        }
    }
}

/// The agent runs: the proof the updater waits for, while an update is recorded.
pub(crate) fn started() {
    let Ok(directory) = config::config_directory() else {
        return;
    };
    match apply::started(&directory, env!("CARGO_PKG_VERSION")) {
        Ok(true) => tracing::info!("the agent's update is proven by this start"),
        Ok(false) => {}
        Err(error) => tracing::warn!(
            error = format!("{error:#}"),
            "the agent's start could not be recorded for its update"
        ),
    }
}
