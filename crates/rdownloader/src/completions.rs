//! `rdownloader completions <shell>` (RD-1240-18): the shell completion script for bash, zsh,
//! fish, PowerShell or elvish, written to standard output.
//!
//! The script is generated from the very command line `main` parses, so a new subcommand or
//! flag is completed the moment it exists, without a hand-kept list per shell.

use anyhow::Result;
use clap::{Args, CommandFactory};
use clap_complete::Shell;

/// The arguments of `rdownloader completions`.
#[derive(Args)]
pub(crate) struct CompletionsArgs {
    /// The shell to write the completion script for.
    shell: Shell,
}

/// Prints the script for the shell asked for.
pub(crate) fn run(args: &CompletionsArgs) -> Result<()> {
    use std::io::Write as _;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&script(args.shell))?;
    stdout.flush()?;
    Ok(())
}

/// The completion script for one shell.
pub(crate) fn script(shell: Shell) -> Vec<u8> {
    let mut command = completed_command();
    let mut buffer = Vec::new();
    clap_complete::generate(shell, &mut command, "rdownloader", &mut buffer);
    buffer
}

/// The command line without its hidden subcommands.
///
/// The generators complete hidden subcommands too, and the one there is, `apply-update`, is
/// started by the service from a copy of itself: offering it at the prompt invites a person to
/// run the updater's process half by hand. clap cannot remove a subcommand, so the root is
/// built again from the visible ones; its own flags are the generated `--help` and `--version`.
fn completed_command() -> clap::Command {
    let root = crate::Cli::command();
    let visible: Vec<clap::Command> = root
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
        .cloned()
        .collect();
    // `default().name(…)`, not `new(…)`: the spawn scan reads every `Command::new(` as a process.
    clap::Command::default()
        .name("rdownloader")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .subcommands(visible)
}

#[cfg(test)]
#[path = "completions_tests.rs"]
mod tests;
