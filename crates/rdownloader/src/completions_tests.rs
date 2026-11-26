use clap::{CommandFactory, ValueEnum};
use clap_complete::Shell;

use super::{completed_command, script};

/// The visible top-level commands, in the order `--help` lists them: the snapshot of what the
/// completion scripts offer at the first word. A new command has to be added here on purpose,
/// and a hidden one never is.
const COMPLETED_COMMANDS: [&str; 13] = [
    "serve",
    "doctor",
    "openapi",
    "plugin",
    "tools",
    "update",
    "autostart",
    "queue",
    "links",
    "events",
    "stop",
    "auth",
    "completions",
];

#[test]
fn the_command_line_is_consistent() {
    crate::Cli::command().debug_assert();
    completed_command().debug_assert();
}

#[test]
fn the_completed_commands_are_the_visible_ones() {
    let completed: Vec<String> = completed_command()
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_owned())
        .collect();
    assert_eq!(completed, COMPLETED_COMMANDS);
    assert!(
        crate::Cli::command()
            .get_subcommands()
            .any(|subcommand| subcommand.get_name() == "apply-update"),
        "the hidden updater command is still part of the command line itself"
    );
}

/// Every name at every depth, so a nested subcommand (`queue list`, `plugin keygen`) is checked
/// as well as the first word.
fn every_name(command: &clap::Command, names: &mut Vec<String>) {
    for subcommand in command.get_subcommands() {
        names.push(subcommand.get_name().to_owned());
        every_name(subcommand, names);
    }
}

#[test]
fn every_shell_completes_every_command_and_never_the_hidden_one() {
    let mut names = Vec::new();
    every_name(&completed_command(), &mut names);
    for shell in Shell::value_variants() {
        let text = String::from_utf8(script(*shell)).expect("a completion script is UTF-8");
        assert!(text.contains("rdownloader"), "{shell}: no binary name");
        for name in &names {
            assert!(
                text.contains(name.as_str()),
                "{shell} does not complete {name}"
            );
        }
        for flag in ["server", "token", "json"] {
            // fish names a long option with `-l`, every other shell writes it out.
            let written = match shell {
                Shell::Fish => format!("-l {flag}"),
                _ => format!("--{flag}"),
            };
            assert!(
                text.contains(&written),
                "{shell} does not complete {written}"
            );
        }
        assert!(
            !text.contains("apply-update"),
            "{shell} offers the hidden updater command"
        );
        assert_eq!(
            script(*shell),
            text.as_bytes(),
            "{shell}: the script is not the same twice"
        );
    }
}

/// The scripts are read by the shells themselves, so a syntax error would only show once
/// somebody sourced one.
#[cfg(unix)]
fn parses(shell: Shell, program: &str) {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join(format!("rdownloader.{program}"));
    std::fs::write(&path, script(shell)).expect("write the script");
    let output = std::process::Command::new(program)
        .arg("-n")
        .arg(&path)
        .output()
        .unwrap_or_else(|error| panic!("{program} did not start: {error}"));
    assert!(
        output.status.success(),
        "{program} -n refused the script: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every system the tests run on has bash: the repository's own scripts need it.
#[cfg(unix)]
#[test]
fn the_bash_script_parses() {
    parses(Shell::Bash, "bash");
}

/// `#[ignore]` and a failure without zsh (RD-1480-14): the test passed unrun where it is missing.
/// CI's Linux leg installs zsh and runs it (`scripts/ci-rust-tests.sh`).
#[cfg(unix)]
#[test]
#[ignore = "needs zsh on PATH"]
fn the_zsh_script_parses() {
    parses(Shell::Zsh, "zsh");
}
