//! `rdownloader-capture update` (RD-1210-03): the agent's own update from a terminal — check,
//! install and wait for the outcome, or switch the agent's own check, the service's request and
//! the automatic install (RD-1240-27).

use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use rd_update::agent::AgentSetup;
use rd_update::agent::apply::START_TIMEOUT_SECS;
use rd_update::install::{Journal, Phase};
use rd_update::{UpdateAction, check::OFFICIAL_REPOSITORY};

use super::{Config, State, check_now, watch::install};
use crate::config;

#[derive(Args)]
pub(crate) struct UpdateArgs {
    /// Only checks and says what is offered; installs nothing.
    #[arg(long)]
    pub(crate) check: bool,
    /// Switches the agent's own check after its start and then daily on or off.
    #[arg(long, value_name = "on|off", conflicts_with = "check")]
    pub(crate) auto_check: Option<Switch>,
    /// Lets the service ask this agent to install an update (off by default).
    #[arg(long, value_name = "on|off", conflicts_with = "check")]
    pub(crate) allow_remote: Option<Switch>,
    /// Installs an update the agent's own check finds by itself (off by default). Only an agent
    /// from the portable archive installed without the service installs itself.
    #[arg(long, value_name = "on|off", conflicts_with = "check")]
    pub(crate) auto_install: Option<Switch>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Switch {
    On,
    Off,
}

pub(crate) async fn update(args: UpdateArgs) -> Result<()> {
    let directory = config::config_directory()?;
    if args.auto_check.is_some() || args.allow_remote.is_some() || args.auto_install.is_some() {
        let mut settings = Config::load(&directory);
        if let Some(switch) = args.auto_check {
            settings.check = switch == Switch::On;
        }
        if let Some(switch) = args.allow_remote {
            settings.allow_remote = switch == Switch::On;
        }
        if let Some(switch) = args.auto_install {
            settings.auto_install = switch == Switch::On;
        }
        settings.store(&directory)?;
        let word = |on: bool| if on { "on" } else { "off" };
        println!(
            "Own update check: {}. Installing at the service's request: {}. Installing updates \
             automatically: {}. A running agent applies this at its next start.",
            word(settings.check),
            word(settings.allow_remote),
            word(settings.auto_install)
        );
        if settings.auto_install && AgentSetup::detect() == AgentSetup::WithService {
            println!(
                "This agent sits beside rDownloader's service, which updates both programs; its \
                 own setting decides."
            );
        }
        return Ok(());
    }
    let running = env!("CARGO_PKG_VERSION");
    let setup = AgentSetup::detect();
    if setup == AgentSetup::WithService {
        println!(
            "rdownloader-capture {running} sits beside rDownloader's service, which updates both \
             programs; there is nothing to do here."
        );
        return Ok(());
    }
    let mut state = State::load(&directory);
    let checked = check_now(setup, None, &mut state, running).await;
    if let Err(error) = state.store(&directory) {
        tracing::warn!(%error, "the agent's update state could not be stored");
    }
    if let Err(error) = checked {
        bail!("the update check cannot run ({}): {error}", error.code());
    }
    if let Some(code) = &state.last_error {
        eprintln!("The check reported a problem: {code}");
    }
    let Some(offer) = state.current_offer(running).cloned() else {
        println!("rdownloader-capture {running} is up to date.");
        return Ok(());
    };
    println!(
        "rdownloader-capture {} is available (this is {running}).",
        offer.version
    );
    match setup.action(&offer.version) {
        Some(UpdateAction::Install) if offer.artifact.is_some() => {}
        Some(UpdateAction::Command { command, .. }) => {
            println!("Update it with: {command}");
            return Ok(());
        }
        _ => {
            println!(
                "Download it from https://github.com/{OFFICIAL_REPOSITORY}/releases/tag/v{}",
                offer.version
            );
            return Ok(());
        }
    }
    if args.check {
        println!("Run `rdownloader-capture update` to install it.");
        return Ok(());
    }
    let version = install(setup, &state, &directory, running, Vec::new())
        .await
        .map_err(|error| anyhow::anyhow!("{} ({})", error.detail, error.code))?;
    println!("Installing {version}; a running agent restarts as the new version.");
    wait_for(&directory, &version).await
}

/// Follows the journal until the update has ended, and says how.
async fn wait_for(directory: &std::path::Path, version: &str) -> Result<()> {
    for _ in 0..(START_TIMEOUT_SECS + 90) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let Some(journal) = Journal::read(directory)? else {
            continue;
        };
        match journal.phase {
            Phase::Verified => {
                println!("rdownloader-capture {version} is installed.");
                return Ok(());
            }
            Phase::RolledBack | Phase::Failed => bail!(
                "the update to {version} did not go ahead and the previous version stays ({})",
                journal.reason.as_deref().unwrap_or("update.failed")
            ),
            _ => {}
        }
    }
    bail!("the update to {version} has not ended yet; its log is in the configuration directory")
}
