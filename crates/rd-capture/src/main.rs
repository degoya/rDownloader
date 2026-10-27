//! Cross-platform desktop capture bridge for CNL, clipboard and NZB files.

#![warn(unreachable_pub)]

mod activity;
mod agent_run;
mod agent_settings;
mod autostart_toggle;
mod cli;
mod client;
mod clipboard;
mod cnl;
mod commands;
mod config;
mod controls;
mod desktop_shortcut;
mod game_mode;
mod hand_over;
mod hotkeys;
// The badge the tray draws over its icon is plain arithmetic on an RGBA buffer, so it carries the
// tray's gate plus `test` and is measured here; only the decoding and `Icon::from_rgba` around it
// need the platform crates.
#[cfg(any(windows, target_os = "macos", test))]
mod icon;
mod instance;
// The link page on the Click'n'Load port (RD-1390-02) and the two phases of a run around it.
mod link_page;
mod link_phase;
mod linkgrabber;
mod notify;
#[cfg(test)]
mod notify_resume;
mod os_integration;
mod relaunch;
mod self_update;
mod server_update;
// The platform gate: what a Linux build of the agent may link, and which modules may be
// compiled there at all. Its own file because it is a check, not a part of the agent.
#[cfg(test)]
mod platform_gate;
mod scheme;
mod sse;
// The rules the tray shows are ordinary Rust and are tested on every host; only the tray that
// draws them is platform-bound. The module therefore carries the tray's own gate plus `test`.
#[cfg(any(windows, target_os = "macos", test))]
mod status;
mod supervision;
#[cfg(any(windows, target_os = "macos"))]
mod tray;
// Every entry of the tray menu and its command (RD-1240-24): a list the tray builds from, held by
// a test on every host. Same gate as `status`.
#[cfg(any(windows, target_os = "macos", test))]
mod tray_menu;
// What the tray shows -- which mark, whether "Open" can be chosen, the status line -- is a rule
// over the service state, the transfer poll and the agent's notices, and it is decided and
// tested here; `tray` only applies the result to its handles. Same gate as `status`.
#[cfg(any(windows, target_os = "macos", test))]
mod tray_state;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use crate::{
    cli::{Cli, Command, RunArgs},
    desktop_shortcut::Place,
    link_phase::{LinkStatus, run},
    supervision::NoticeSink,
};

// `main` is deliberately synchronous: the tray needs a platform UI event loop on
// the main thread, so the tokio runtime is created explicitly per subcommand.
fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    match dispatch() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => std::process::ExitCode::from(conclude(&error)),
    }
}

/// How the process ends after the agent stopped with `error`: as the program that replaced this
/// one when that is why it stopped ([`relaunch`]), otherwise with [`report`]'s account and code.
/// Shared with the tray path, like `report`.
pub(crate) fn conclude(error: &anyhow::Error) -> u8 {
    if let Some(replaced) = error.downcast_ref::<relaunch::Replaced>() {
        return relaunch::relaunch(replaced);
    }
    report(error)
}

/// Prints the agent's account of a failure and returns the code to end the process on.
///
/// Shared with the tray path on purpose. On Windows the agent runs under the tray's event loop,
/// where every failure used to end on 1 — so the code below, which exists precisely so a
/// launcher can tell "already running" apart from "crashed", was unreachable on the one platform
/// whose launcher reads it. An agent without a token does not end any more; it runs unlinked
/// (RD-1390-02).
pub(crate) fn report(error: &anyhow::Error) -> u8 {
    // An ordinary state, not a fault, so it says what to do instead of printing an error chain.
    if let Some(busy) = error.downcast_ref::<PortBusy>() {
        eprintln!(
            "{busy}. Another Click'n'Load listener already has the port — a second \
             rdownloader-capture (check your autostart) or JDownloader. This agent is not \
             needed while that one is running; `rdownloader-capture link` opens the link page \
             of the one that runs."
        );
        return config::EXIT_PORT_BUSY;
    }
    // The other "already running", told apart by the lock rather than the port (RD-1200-03).
    if error.is::<instance::AlreadyRunning>() {
        eprintln!(
            "{}. This agent is not needed while that one is running.",
            instance::AlreadyRunning
        );
        return config::EXIT_PORT_BUSY;
    }
    eprintln!("Error: {error:?}");
    1
}

/// Marker for "every Click'n'Load address was already taken", carried by the error chain.
///
/// Carries the addresses rather than naming 9666: they come from `--cnl-listen`, and a message
/// that states the port it actually tried is the one worth reading in a log.
#[derive(Debug)]
struct PortBusy {
    addresses: Vec<SocketAddr>,
}

impl std::fmt::Display for PortBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Click'n'Load could not bind any of ")?;
        for (index, address) in self.addresses.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            write!(formatter, "{address}")?;
        }
        Ok(())
    }
}

impl std::error::Error for PortBusy {}

fn dispatch() -> Result<()> {
    match Cli::parse().command {
        Some(Command::Run(args)) => guarded_run(args),
        None => guarded_run(cli::default_run_args()),
        Some(Command::Open(args)) => runtime()?.block_on(commands::open(args)),
        Some(Command::Handle(args)) => runtime()?.block_on(commands::handle(args)),
        Some(Command::Configure(args)) => commands::configure(args),
        Some(Command::Link(args)) => commands::link(args),
        Some(Command::Association(args)) => {
            commands::integration(args, os_integration::Kind::Association)
        }
        Some(Command::Scheme(args)) => commands::integration(args, os_integration::Kind::Scheme),
        Some(Command::DropShortcut(args)) => commands::shortcut(args, Place::Desktop),
        Some(Command::SendTo(args)) => commands::shortcut(args, Place::SendTo),
        Some(Command::Autostart(args)) => commands::autostart(args),
        Some(Command::Pause(args)) => runtime()?.block_on(commands::clipboard_watch(args, true)),
        Some(Command::Resume(args)) => runtime()?.block_on(commands::clipboard_watch(args, false)),
        Some(Command::Status(args)) => runtime()?.block_on(commands::status(args)),
        Some(Command::SendClipboard(args)) => runtime()?.block_on(commands::send_clipboard(args)),
        Some(Command::Update(args)) => runtime()?.block_on(self_update::update(args)),
        Some(Command::ApplyUpdate(args)) => self_update::apply_update(args),
    }
}

/// Runs the agent, linked or not: without a token it waits for a link on its page (RD-1390-02).
fn guarded_run(args: RunArgs) -> Result<()> {
    // Held until the process ends; the tray path never returns from `run_agent`.
    let _instance = instance::acquire(&config::config_directory()?, instance::RELAUNCH_WAIT)?;
    // An update of the agent's own that a stop interrupted ends here (RD-1210-03).
    if self_update::recover_at_start() {
        return Ok(());
    }
    run_agent(args)
}

/// The multi-threaded runtime `#[tokio::main]` used to build for us.
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Runtime::new().context("start the asynchronous runtime")
}

/// Runs the agent with a tray icon where the platform has one, otherwise
/// headless. A tray that cannot be initialised degrades to headless.
#[cfg(any(windows, target_os = "macos"))]
fn run_agent(args: RunArgs) -> Result<()> {
    if args.no_tray {
        return run_headless(args);
    }
    match tray::prepare() {
        Ok(tray) => tray.run(args),
        Err(error) => {
            tracing::warn!(%error, "tray icon unavailable; running headless");
            run_headless(args)
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_agent(args: RunArgs) -> Result<()> {
    if args.no_tray {
        tracing::debug!("--no-tray has no effect; this platform is always headless");
    }
    run_headless(args)
}

fn run_headless(args: RunArgs) -> Result<()> {
    runtime()?.block_on(run(args, CancellationToken::new(), None))
}

/// Reports transfer activity to whatever is showing it; `None` when nothing is (headless runs).
///
/// A callback rather than a channel so the tray can hand its event-loop proxy straight in, and
/// so this signature says nothing about tao, which does not exist on Linux.
pub(crate) type ActivitySink = std::sync::Arc<dyn Fn(activity::Activity) + Send + Sync>;

/// What a desktop run has that a headless one does not: somewhere to draw.
///
/// Passed as one value rather than as independent options because they arrive together —
/// there is exactly one event loop, and either both sinks lead to it or neither exists.
pub(crate) struct DesktopSinks {
    pub activity: ActivitySink,
    pub notice: NoticeSink,
    /// The clipboard pause and the shortcuts, for the tray's check mark, icon, accelerators and
    /// registrations (RD-1180-01, RD-1180-03).
    pub settings: controls::SettingsSink,
    /// What the tray menu and its shortcuts ask of the agent (RD-1100-06). The other direction:
    /// the tray names the request, the agent makes it, because the agent holds the token.
    pub inbox: controls::Inbox,
    /// The tray's entry for the agent's own update (RD-1210-03).
    pub update: self_update::OfferSink,
    /// The tray's entry for the service's update and its server line (RD-1240-25).
    pub server_update: server_update::ViewSink,
    /// Whether the agent is linked, and with what, for the tray's lines and its "Link capture
    /// agent" entry (RD-1390-02).
    pub link: tokio::sync::watch::Sender<LinkStatus>,
}

impl DesktopSinks {
    /// The same sinks with another inbox: what one linked phase is handed, while the tray's own
    /// inbox stays with the run and is relayed to it (RD-1390-02).
    pub(crate) fn with_inbox(&self, inbox: controls::Inbox) -> Self {
        Self {
            activity: self.activity.clone(),
            notice: self.notice.clone(),
            settings: self.settings.clone(),
            inbox,
            update: self.update.clone(),
            server_update: self.server_update.clone(),
            link: self.link.clone(),
        }
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod cli_tests;
