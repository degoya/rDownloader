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
    /// Hands files to rDownloader: NZBs, link containers (.dlc, .ccf, .rsdf, .rdlinks,
    /// .crawljob) and torrents, as a file association, the desktop drop shortcut and Send to
    /// start it; several at once end in one notification.
    Open(OpenArgs),
    /// Handles one `rdownloader://` address handed over by the desktop.
    Handle(HandleArgs),
    /// Registers or removes the `rdownloader://` URL scheme handler.
    Scheme(IntegrationArgs),
    /// Saves the paired service URL and capture token for autostart.
    Configure(ConfigureArgs),
    /// Opens the link page of the running agent, where a server address and a token link it
    /// without a shell (RD-1390-02); `--print` prints the page's address instead.
    Link(LinkArgs),
    /// Installs or removes the per-user file association: `.nzb`, and "Import with rDownloader"
    /// for the link containers and `.torrent` without taking another program's default.
    Association(IntegrationArgs),
    /// Puts or removes the "Add to rDownloader" shortcut on the desktop (Windows): files dropped
    /// on it are handed over.
    DropShortcut(IntegrationArgs),
    /// Puts or removes "rDownloader" in the Send to menu (Windows).
    SendTo(IntegrationArgs),
    /// Installs or removes per-user capture-agent autostart.
    Autostart(IntegrationArgs),
    /// Pauses clipboard watching. Click'n'Load, the browser extension and rdownloader:// links
    /// stay on; what is copied while paused is not delivered later.
    Pause(ConnectionArgs),
    /// Resumes clipboard watching.
    Resume(ConnectionArgs),
    /// Says whether clipboard watching is paused, and lists the shortcuts.
    Status(ConnectionArgs),
    /// Reads the clipboard once and hands its links over, also while watching is paused.
    SendClipboard(ConnectionArgs),
    /// Checks for a newer agent and installs it, for an agent installed without rDownloader's
    /// service; or switches its own check and the service's request on or off.
    Update(crate::self_update::UpdateArgs),
    /// The agent's updater, which the agent starts from a copy of itself.
    #[command(hide = true)]
    ApplyUpdate(crate::self_update::ApplyArgs),
}

#[derive(Args, Clone)]
pub(crate) struct ConnectionArgs {
    #[arg(long, env = "RDOWNLOADER_SERVICE")]
    pub(crate) service: Option<Url>,
    /// The keyring, the fallback file or `RDOWNLOADER_CAPTURE_TOKEN` -- never an argument:
    /// every local account reads `argv` through `ps`, and the relaunch after an update repeats
    /// it (RD-1190-22).
    #[arg(skip = std::env::var("RDOWNLOADER_CAPTURE_TOKEN").ok())]
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

#[derive(Args, Clone)]
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
    /// Never opens the link page by itself, not even at the first start without a link; the
    /// tray's "Link capture agent" and `link` still open it (RD-1390-02).
    #[arg(long)]
    pub(crate) no_link_page: bool,
}

#[derive(Args)]
pub(crate) struct LinkArgs {
    /// Prints the page's address rather than opening it, for a desktop without a browser to
    /// open or for a script.
    #[arg(long)]
    pub(crate) print: bool,
}

/// `open` and `handle` hand over to the service themselves, with the pairing token, rather than
/// through the Click'n'Load port, which another account can hold while the agent is not running
/// (RD-1200-03).
#[derive(Args)]
pub(crate) struct OpenArgs {
    #[command(flatten)]
    pub(crate) connection: ConnectionArgs,
    /// The files, as the desktop passes them: one for a double-click, several for a drop on the
    /// shortcut or Send to (RD-1260-09).
    #[arg(required = true)]
    pub(crate) paths: Vec<PathBuf>,
}

#[derive(Args)]
pub(crate) struct HandleArgs {
    #[command(flatten)]
    pub(crate) connection: ConnectionArgs,
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
        no_link_page: false,
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
