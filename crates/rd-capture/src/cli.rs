//! The command line the agent is started with: every clap type it parses, and the defaults a
//! bare invocation runs with.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
};

use clap::{Args, Parser, Subcommand};
use url::Url;

use crate::config;

#[derive(Parser)]
#[command(name = "rdownloader-capture", version)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Runs Click'n'Load and optional clipboard monitoring.
    Run(RunArgs),
    /// Imports an NZB selected through an operating-system file association.
    Open(OpenArgs),
    /// Handles one `rdownloader://` address handed over by the desktop.
    Handle(HandleArgs),
    /// Registers or removes the `rdownloader://` URL scheme handler.
    Scheme(IntegrationArgs),
    /// Saves the paired service URL and capture token for autostart.
    Configure(ConfigureArgs),
    /// Installs or removes the per-user `.nzb` file association.
    Association(IntegrationArgs),
    /// Installs or removes per-user capture-agent autostart.
    Autostart(IntegrationArgs),
}

#[derive(Args)]
pub(crate) struct ConnectionArgs {
    #[arg(long, env = "RDOWNLOADER_SERVICE")]
    pub(crate) service: Option<Url>,
    #[arg(long, env = "RDOWNLOADER_CAPTURE_TOKEN")]
    pub(crate) token: Option<String>,
}

#[derive(Args)]
pub(crate) struct ConfigureArgs {
    #[arg(long, default_value = config::DEFAULT_SERVICE)]
    pub(crate) service: Url,
    /// The capture token. Readable in `ps` and kept in the shell history; prefer
    /// `--token-stdin`.
    #[arg(
        long,
        conflicts_with = "token_stdin",
        required_unless_present = "token_stdin"
    )]
    pub(crate) token: Option<String>,
    /// Reads the capture token from standard input instead of the command line.
    ///
    /// The documented way: an argument is visible to every local user through `ps` or the task
    /// manager, and a shell writes it to its history besides (RD-109-04).
    #[arg(long)]
    pub(crate) token_stdin: bool,
    /// Pairs with a service that is neither loopback nor `https`, accepting that the capture
    /// token travels the network unencrypted.
    #[arg(long)]
    pub(crate) allow_insecure_service: bool,
}

#[derive(Args)]
pub(crate) struct RunArgs {
    #[command(flatten)]
    pub(crate) connection: ConnectionArgs,
    #[arg(
        long = "cnl-listen",
        default_values = ["127.0.0.1:9666", "[::1]:9666"]
    )]
    pub(crate) cnl_listen: Vec<SocketAddr>,
    // Clipboard monitoring is part of the normal agent mode. Keeping the old
    // --clipboard switch accepted preserves existing autostart registrations.
    #[arg(long, default_value_t = true)]
    pub(crate) clipboard: bool,
    /// Runs without desktop notifications for links arriving in the LinkGrabber.
    // Same falsey parser as `--no-tray`, so `RDOWNLOADER_NO_NOTIFICATIONS=1` works.
    #[arg(
        long,
        env = "RDOWNLOADER_NO_NOTIFICATIONS",
        value_parser = clap::builder::FalseyValueParser::new()
    )]
    pub(crate) no_notifications: bool,
    /// Runs without the tray/menu-bar icon. Always headless on Linux.
    // clap's default bool parser only accepts the literal strings "true"/"false", so
    // `RDOWNLOADER_NO_TRAY=1` (documented in the README) would fail with "invalid value '1'"
    // and exit 2. FalseyValueParser also accepts 1/0/empty/yes/no/on/off, matching the README.
    #[arg(
        long,
        env = "RDOWNLOADER_NO_TRAY",
        value_parser = clap::builder::FalseyValueParser::new()
    )]
    pub(crate) no_tray: bool,
}

#[derive(Args)]
pub(crate) struct OpenArgs {
    #[arg(long, default_value = "http://127.0.0.1:9666")]
    pub(crate) agent: Url,
    pub(crate) path: PathBuf,
}

#[derive(Args)]
pub(crate) struct HandleArgs {
    #[arg(long, default_value = "http://127.0.0.1:9666")]
    pub(crate) agent: Url,
    /// The `rdownloader://` address.
    pub(crate) url: String,
}

#[derive(Args)]
pub(crate) struct IntegrationArgs {
    #[command(subcommand)]
    pub(crate) command: IntegrationCommand,
}

#[derive(Subcommand)]
pub(crate) enum IntegrationCommand {
    /// Registers the integration for the next user login or file open.
    Install,
    /// Removes the per-user integration registration.
    Remove,
}

pub(crate) fn default_run_args() -> RunArgs {
    RunArgs {
        connection: ConnectionArgs {
            service: None,
            token: None,
        },
        cnl_listen: vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9666),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 9666),
        ],
        clipboard: true,
        no_notifications: false,
        no_tray: false,
    }
}

#[cfg(test)]
mod tests {
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
}
