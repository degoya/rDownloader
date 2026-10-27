//! The command line's parsing (`cli.rs`): what each subcommand takes and what it refuses.

use clap::{CommandFactory, Parser};

use super::{Cli, Command, RunArgs};

fn run_args(arguments: &[&str]) -> RunArgs {
    let cli = Cli::try_parse_from(arguments).expect("valid CLI");
    let Some(Command::Run(args)) = cli.command else {
        panic!("run command expected");
    };
    args
}

/// A token in `argv` is readable by every local user through `ps` and lands in the shell
/// history besides. There has to be a way that is neither (RD-109-04).
#[test]
fn the_token_can_be_given_without_putting_it_in_the_command_line() {
    let stdin_form = Cli::try_parse_from(["rdownloader-capture", "configure", "--token-stdin"])
        .expect("--token-stdin alone is a complete invocation");
    let Some(Command::Configure(args)) = stdin_form.command else {
        panic!("configure command expected");
    };
    assert!(args.token_stdin);
    assert_eq!(args.token, None);

    // The argument form still parses, so nobody's script breaks.
    assert!(Cli::try_parse_from(["rdownloader-capture", "configure", "--token", "x"]).is_ok());
    // Giving both would leave it ambiguous which one counts.
    assert!(
        Cli::try_parse_from([
            "rdownloader-capture",
            "configure",
            "--token",
            "x",
            "--token-stdin"
        ])
        .is_err()
    );
    // Neither is still an error rather than a silent empty token.
    assert!(Cli::try_parse_from(["rdownloader-capture", "configure"]).is_err());
    // The named way out of the https rule sits on the same subcommand.
    assert!(
        Cli::try_parse_from([
            "rdownloader-capture",
            "configure",
            "--token-stdin",
            "--service",
            "http://nas.example",
            "--allow-insecure-service"
        ])
        .is_ok()
    );
}

/// The tray's clipboard commands on a desktop without a tray (RD-1180-01, RD-1180-03).
#[test]
fn the_clipboard_commands_parse_with_the_connection_of_every_other_command() {
    for (word, expected) in [
        ("pause", "pause"),
        ("resume", "resume"),
        ("status", "status"),
        ("send-clipboard", "send-clipboard"),
    ] {
        let cli = Cli::try_parse_from([
            "rdownloader-capture",
            word,
            "--service",
            "http://127.0.0.1:8710",
        ])
        .unwrap_or_else(|error| panic!("{word}: {error}"));
        let (name, connection) = match cli.command {
            Some(Command::Pause(connection)) => ("pause", connection),
            Some(Command::Resume(connection)) => ("resume", connection),
            Some(Command::Status(connection)) => ("status", connection),
            Some(Command::SendClipboard(connection)) => ("send-clipboard", connection),
            _ => panic!("{word}: a clipboard command expected"),
        };
        assert_eq!(name, expected);
        assert_eq!(
            connection.service.map(|service| service.to_string()),
            Some("http://127.0.0.1:8710/".to_owned())
        );
    }
    assert!(Cli::try_parse_from(["rdownloader-capture", "pause"]).is_ok());
    assert!(Cli::try_parse_from(["rdownloader-capture", "pause", "now"]).is_err());
}

/// RD-1190-22: `run --token` put the token in `ps`, and the relaunch after an update kept
/// it there. No command that connects takes it as an argument any more.
#[test]
fn no_connecting_command_takes_the_token_as_an_argument() {
    for word in ["run", "pause", "resume", "status", "send-clipboard"] {
        assert!(
            Cli::try_parse_from(["rdownloader-capture", word, "--token", "x"]).is_err(),
            "{word}"
        );
    }
}

/// `open` and `handle` hand over to the service themselves (RD-1200-03): they take the
/// connection of every other command, and no address on the Click'n'Load port any more.
#[test]
fn open_and_handle_connect_to_the_service_rather_than_to_the_port() {
    let cli = Cli::try_parse_from([
        "rdownloader-capture",
        "open",
        "--service",
        "http://127.0.0.1:8710",
        "release.nzb",
    ])
    .expect("open parses");
    let Some(Command::Open(args)) = cli.command else {
        panic!("open command expected");
    };
    assert_eq!(
        args.connection.service.map(|service| service.to_string()),
        Some("http://127.0.0.1:8710/".to_owned())
    );
    assert!(Cli::try_parse_from(["rdownloader-capture", "handle", "rdownloader://x"]).is_ok());
    for word in ["open", "handle"] {
        for flag in ["--agent", "--token"] {
            assert!(
                Cli::try_parse_from(["rdownloader-capture", word, flag, "x", "y"]).is_err(),
                "{word} {flag}"
            );
        }
    }
}

/// RD-1260-09: what Windows passes for a drop on the shortcut or Send to -- every file after
/// `open` -- and the two shortcut commands.
#[test]
fn open_takes_every_dropped_file_and_the_shortcuts_parse() {
    let cli = Cli::try_parse_from([
        "rdownloader-capture",
        "open",
        r"C:\Downloads\a.dlc",
        r"C:\Downloads\b c.nzb",
    ])
    .expect("open parses");
    let Some(Command::Open(args)) = cli.command else {
        panic!("open command expected");
    };
    assert_eq!(args.paths.len(), 2);
    assert!(Cli::try_parse_from(["rdownloader-capture", "open"]).is_err());
    for word in ["drop-shortcut", "send-to"] {
        for action in ["install", "remove"] {
            assert!(
                Cli::try_parse_from(["rdownloader-capture", word, action]).is_ok(),
                "{word} {action}"
            );
        }
    }
}

/// RD-1210-03: `update` checks and installs, or switches; `apply-update` is the updater's own.
#[test]
fn the_update_commands_parse() {
    for argv in [
        &["rdownloader-capture", "update"][..],
        &["rdownloader-capture", "update", "--check"],
        &["rdownloader-capture", "update", "--auto-check", "off"],
        &["rdownloader-capture", "update", "--allow-remote", "on"],
        &[
            "rdownloader-capture",
            "apply-update",
            "--journal",
            "/tmp/j.json",
        ],
    ] {
        assert!(Cli::try_parse_from(argv).is_ok(), "{argv:?}");
    }
    for argv in [
        &["rdownloader-capture", "update", "--auto-check", "maybe"][..],
        &[
            "rdownloader-capture",
            "update",
            "--check",
            "--allow-remote",
            "on",
        ],
        &["rdownloader-capture", "apply-update"],
    ] {
        assert!(Cli::try_parse_from(argv).is_err(), "{argv:?}");
    }
}

#[test]
fn explicit_run_enables_clipboard_monitoring_by_default() {
    assert!(run_args(&["rdownloader-capture", "run"]).clipboard);
}

#[test]
fn tray_is_the_default_and_no_tray_switches_it_off() {
    assert!(!run_args(&["rdownloader-capture", "run"]).no_tray);
    assert!(run_args(&["rdownloader-capture", "run", "--no-tray"]).no_tray);
    assert!(!super::default_run_args().no_tray);
}

#[test]
fn notifications_are_on_by_default_and_switchable_off() {
    assert!(!run_args(&["rdownloader-capture", "run"]).no_notifications);
    assert!(run_args(&["rdownloader-capture", "run", "--no-notifications"]).no_notifications);
    assert!(!super::default_run_args().no_notifications);
}

#[test]
fn no_notifications_takes_the_same_falsey_environment_forms_as_no_tray() {
    let command = Cli::command();
    let run = command.find_subcommand("run").expect("run subcommand");
    let argument = run
        .get_arguments()
        .find(|argument| argument.get_id() == "no_notifications")
        .expect("--no-notifications argument");
    assert_eq!(
        argument.get_env().and_then(|name| name.to_str()),
        Some("RDOWNLOADER_NO_NOTIFICATIONS")
    );
    // Same reasoning as the `no_tray` case below: this fails if the falsey parser is ever
    // dropped, which would reject the documented `=1` form.
    let accepted: Vec<String> = argument
        .get_value_parser()
        .possible_values()
        .expect("no_notifications has an enumerable value parser")
        .map(|value| value.get_name().to_owned())
        .collect();
    assert!(
        accepted.iter().any(|value| value == "1"),
        "no_notifications must accept \"1\", got {accepted:?}"
    );
}

#[test]
fn no_tray_is_also_readable_from_the_environment() {
    // Asserted through the parser definition rather than by setting the
    // variable: `std::env::set_var` is unsafe in edition 2024 and would
    // leak into every other test running in the same process.
    let command = Cli::command();
    let run = command.find_subcommand("run").expect("run subcommand");
    let argument = run
        .get_arguments()
        .find(|argument| argument.get_id() == "no_tray")
        .expect("--no-tray argument");
    assert_eq!(
        argument.get_env().and_then(|name| name.to_str()),
        Some("RDOWNLOADER_NO_TRAY")
    );

    // The env value is parsed with the *installed* `--no-tray` value_parser, so this
    // proves the falsey parser (not clap's default strict "true"/"false" bool parser) is
    // actually wired onto the argument: `BoolValueParser` (clap's default for a plain
    // `bool` field) only ever advertises `["true", "false"]`, so this assertion fails if
    // `value_parser = clap::builder::FalseyValueParser::new()` is ever removed from the
    // `#[arg(...)]` attribute — the documented `RDOWNLOADER_NO_TRAY=1` form would then be
    // rejected again with clap's "invalid value '1'" error.
    let accepted: Vec<String> = argument
        .get_value_parser()
        .possible_values()
        .expect("no_tray has an enumerable value parser")
        .map(|value| value.get_name().to_owned())
        .collect();
    assert!(
        accepted.iter().any(|value| value == "1"),
        "no_tray's value parser must accept \"1\" (documented in the README), got {accepted:?}"
    );
    assert!(
        accepted.iter().any(|value| value == "0"),
        "no_tray's value parser must accept \"0\", got {accepted:?}"
    );

    // Direct semantics check of the parser type itself, matching the forms documented in
    // the README (`RDOWNLOADER_NO_TRAY=1`, `=0`, `=true`, or unset/empty).
    use clap::builder::TypedValueParser;
    use std::ffi::OsStr;
    let falsey = clap::builder::FalseyValueParser::new();
    assert!(
        falsey
            .parse_ref(&command, Some(argument), OsStr::new("1"))
            .expect("\"1\" parses")
    );
    assert!(
        !falsey
            .parse_ref(&command, Some(argument), OsStr::new("0"))
            .expect("\"0\" parses")
    );
    assert!(
        falsey
            .parse_ref(&command, Some(argument), OsStr::new("true"))
            .expect("\"true\" parses")
    );
    assert!(
        !falsey
            .parse_ref(&command, Some(argument), OsStr::new(""))
            .expect("\"\" parses")
    );
}

/// RD-1390-02: `link` opens or prints the running agent's page, and `run --no-link-page` keeps
/// the page from opening by itself.
#[test]
fn link_parses_and_the_page_can_be_kept_from_opening() {
    for (argv, print) in [
        (&["rdownloader-capture", "link"][..], false),
        (&["rdownloader-capture", "link", "--print"], true),
    ] {
        let cli = Cli::try_parse_from(argv).unwrap_or_else(|error| panic!("{argv:?}: {error}"));
        let Some(Command::Link(args)) = cli.command else {
            panic!("link command expected");
        };
        assert_eq!(args.print, print);
    }
    assert!(Cli::try_parse_from(["rdownloader-capture", "link", "--token", "x"]).is_err());
    assert!(!run_args(&["rdownloader-capture", "run"]).no_link_page);
    assert!(run_args(&["rdownloader-capture", "run", "--no-link-page"]).no_link_page);
    assert!(!super::default_run_args().no_link_page);
}
