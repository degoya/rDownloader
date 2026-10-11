//! The relauncher of a restart (RD-1240-32): `rdownloader restart-service`, the updater's steps
//! without the switch.
//!
//! Hidden from the help: the service starts it from a copy of itself in `<data>/update/updater/`
//! (`rd_update::restart::launch_relauncher`) with the plan it wrote, when it restarts itself
//! rather than leaving that to systemd or a container runtime. It holds the updater lock for its
//! whole life, so a restart and an update never run at once, then:
//!
//! 1. stops the service over the local control token (`rdownloader stop`'s way) and waits until it
//!    has saved its queue and ended; one that accepted the stop and is still running after
//!    [`STOP_WAIT`] is ended by force, by the process id and name the plan records;
//! 2. starts the same executable with the same arguments in the same folder
//!    (`rd_update::install::process::start_program`) and waits until its health route answers
//!    with the same version, as the updater waits for a new one.
//!
//! Exit codes: `0` the service answers again, `1` anything else; `<data>/update/updater.log`
//! says what happened.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use rd_update::RestartPlan;
use rd_update::install::{UpdaterLock, process};

/// How long the service may take to save its queue and end, as for an update.
const STOP_WAIT: Duration = Duration::from_secs(120);

#[derive(Args)]
pub(crate) struct RestartArgs {
    /// The plan the service wrote: `<data>/update/restart.json`.
    #[arg(long)]
    plan: PathBuf,
}

pub(crate) async fn run(args: RestartArgs) -> Result<()> {
    let plan = RestartPlan::read(&args.plan)?;
    plan.validate()?;
    let Some(lock) = UpdaterLock::acquire(&plan.data_dir)? else {
        anyhow::bail!(
            "an updater runs for {}; nothing is restarted",
            plan.data_dir.display()
        );
    };
    let outcome = restart(&plan).await;
    if let Err(error) = std::fs::remove_file(&args.plan) {
        tracing::debug!(%error, "the restart plan was not removed");
    }
    drop(lock);
    match outcome {
        Ok(()) => {
            tracing::info!(version = %plan.version, "rDownloader was restarted and answers");
            Ok(())
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "the restart did not complete");
            std::process::exit(1);
        }
    }
}

async fn restart(plan: &RestartPlan) -> Result<()> {
    tracing::info!(
        pid = plan.service_pid,
        "stopping rDownloader for its restart"
    );
    if let Err(error) = crate::stop_cli::stop(&plan.data_dir, STOP_WAIT).await {
        match error.downcast_ref::<crate::stop_cli::NotEnded>() {
            Some(_) => crate::updater_cli::end_process(plan.service_pid, &plan.image())
                .await
                .map_err(anyhow::Error::msg)?,
            None => return Err(error.context("rDownloader did not stop for its restart")),
        }
    }
    let mut child = process::start_program(&plan.executable, &plan.service_args, &plan.service_cwd)
        .context("start rDownloader again")?;
    crate::updater_cli::await_answer(
        &plan.data_dir,
        plan.health_timeout_secs,
        &mut child,
        &plan.version,
    )
    .await
    .map_err(|(code, detail)| anyhow::anyhow!("{code}: {detail}"))
}
