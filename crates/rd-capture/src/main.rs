//! Cross-platform desktop capture bridge for CNL, clipboard and NZB files.

#![warn(unreachable_pub)]

mod activity;
mod agent_settings;
mod cli;
mod client;
mod clipboard;
mod cnl;
mod commands;
mod config;
mod controls;
mod game_mode;
mod hotkeys;
// The badge the tray draws over its icon is plain arithmetic on an RGBA buffer, so it carries the
// tray's gate plus `test` and is measured here; only the decoding and `Icon::from_rgba` around it
// need the platform crates.
#[cfg(any(windows, target_os = "macos", test))]
mod icon;
mod instance;
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
    client::CaptureClient,
    supervision::{
        AgentNotice, NoticeSink, bind_click_n_load, join_addresses, supervised, wind_down,
    },
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
/// where every failure used to end on 1 — so the two codes below, which exist precisely so a
/// launcher can tell "nothing to do yet" and "already running" apart from "crashed", were
/// unreachable on the one platform whose launcher reads them.
pub(crate) fn report(error: &anyhow::Error) -> u8 {
    // Both of these are ordinary states, not faults, so they say what to do instead of
    // printing an error chain.
    if error.is::<NotPaired>() {
        eprintln!(
            "rdownloader-capture is not paired yet. Open the web interface, go to \
             Settings > Desktop client, and run the command it shows."
        );
        return config::EXIT_NOT_PAIRED;
    }
    if let Some(busy) = error.downcast_ref::<PortBusy>() {
        eprintln!(
            "{busy}. Another Click'n'Load listener already has the port — a second \
             rdownloader-capture (check your autostart) or JDownloader. This agent is not \
             needed while that one is running."
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

/// Marker for "the agent has nothing to connect to yet", carried by the error chain.
#[derive(Debug)]
struct NotPaired;

impl std::fmt::Display for NotPaired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("capture token missing")
    }
}

impl std::error::Error for NotPaired {}

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
        Some(Command::Association(args)) => {
            commands::integration(args, os_integration::Kind::Association)
        }
        Some(Command::Scheme(args)) => commands::integration(args, os_integration::Kind::Scheme),
        Some(Command::Autostart(args)) => commands::autostart(args),
        Some(Command::Pause(args)) => runtime()?.block_on(commands::clipboard_watch(args, true)),
        Some(Command::Resume(args)) => runtime()?.block_on(commands::clipboard_watch(args, false)),
        Some(Command::Status(args)) => runtime()?.block_on(commands::status(args)),
        Some(Command::SendClipboard(args)) => runtime()?.block_on(commands::send_clipboard(args)),
        Some(Command::Update(args)) => runtime()?.block_on(self_update::update(args)),
        Some(Command::ApplyUpdate(args)) => self_update::apply_update(args),
    }
}

/// Runs the agent, or reports that there is nothing to connect to yet.
fn guarded_run(args: RunArgs) -> Result<()> {
    if !config::is_paired(args.connection.token.as_deref()) {
        return Err(anyhow::Error::new(NotPaired));
    }
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
}

async fn run(
    args: RunArgs,
    cancellation: CancellationToken,
    desktop: Option<DesktopSinks>,
) -> Result<()> {
    if args.cnl_listen.is_empty()
        || args
            .cnl_listen
            .iter()
            .any(|address| !address.ip().is_loopback())
    {
        anyhow::bail!("Click'n'Load requires at least one loopback address");
    }
    let connection = config::load(args.connection.service, args.connection.token)?;
    // For the Linux shortcut listener, which opens the web interface itself, and for the server
    // update's page (RD-1240-25).
    let service = connection.service.clone();
    let client = CaptureClient::new(connection.service, connection.token)?;
    let signal = cancellation.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal.cancel();
    });
    let notice = desktop.as_ref().map(|desktop| desktop.notice.clone());
    let offer_sink = desktop.as_ref().map(|desktop| desktop.update.clone());
    let server_update_sink = desktop
        .as_ref()
        .map(|desktop| desktop.server_update.clone());
    // Every background task is held rather than spawned and forgotten, so its end is read
    // (RD-109-07) and so the error path below can give it a moment to stop.
    let mut background = tokio::task::JoinSet::new();
    // The tray holds the senders of its own requests; a run without one has only the shortcuts.
    let (desktop, inbox, headless) = match desktop {
        Some(DesktopSinks {
            activity,
            settings,
            inbox,
            ..
        }) => (Some((activity, settings)), inbox, None),
        None => {
            let (controls, inbox) = controls::channels();
            (None, inbox, Some(controls))
        }
    };
    let controls::Inbox {
        queue,
        settings: settings_requests,
        hand_over,
        self_update: update_requests,
        server_update: server_update_requests,
    } = inbox;
    self_update::spawn(
        &mut background,
        &client,
        &cancellation,
        offer_sink,
        update_requests,
    );
    server_update::spawn(
        &mut background,
        &client,
        service.clone(),
        &cancellation,
        notice.clone(),
        server_update_sink,
        server_update_requests,
    );
    let settings = controls::follow_settings(
        &mut background,
        &client,
        &cancellation,
        notice.clone(),
        settings_requests,
        desktop.as_ref().map(|(_, sink)| sink.clone()),
    );
    // Pausing for a game (RD-1240-19); it does nothing until the settings name a trigger.
    game_mode::spawn(
        &mut background,
        &client,
        &cancellation,
        notice.clone(),
        settings.clone(),
    );
    if args.clipboard {
        tracing::info!("clipboard monitoring enabled");
        background.spawn(supervised(
            "clipboard monitoring",
            cancellation.clone(),
            notice.clone(),
            clipboard::watch_clipboard(
                client.clone(),
                cancellation.clone(),
                settings.clone(),
                hand_over,
            ),
        ));
    }
    if let Some(controls) = headless {
        hotkeys::spawn_headless(
            &mut background,
            controls,
            settings.clone(),
            service,
            &cancellation,
        );
    }
    if args.no_notifications {
        tracing::debug!("desktop notifications disabled");
    } else {
        let (task_client, task_cancellation) = (client.clone(), cancellation.clone());
        background.spawn(supervised(
            "intake notifications",
            cancellation.clone(),
            notice.clone(),
            async move {
                notify::watch_intake(task_client, task_cancellation).await;
                Ok(())
            },
        ));
    }
    // Only when something is displaying it. The polls run on the agent's side rather than
    // beside the health check because they need the capture token, and the tray deliberately
    // never reads the keyring itself — a second lookup risks a second macOS Keychain prompt.
    let (task_client, task_cancellation) = (client.clone(), cancellation.clone());
    if let Some((activity, _)) = desktop {
        background.spawn(supervised(
            "the transfer poll",
            cancellation.clone(),
            notice.clone(),
            async move {
                activity::watch_activity(task_client, task_cancellation, activity, queue).await;
                Ok(())
            },
        ));
    } else {
        // Without a tray nothing reads the summary, but a shortcut may still pause the queue.
        background.spawn(controls::serve_queue_requests(
            task_client,
            task_cancellation,
            queue,
        ));
    }
    let attempted = args.cnl_listen.clone();
    let bindings = bind_click_n_load(args.cnl_listen).await;
    if bindings.listeners.is_empty() {
        cancellation.cancel();
        wind_down(&mut background, &mut tokio::task::JoinSet::new()).await;
        return Err(anyhow::Error::new(PortBusy {
            addresses: attempted,
        }));
    }
    if !bindings.unbound.is_empty() {
        let bound: Vec<SocketAddr> = bindings
            .listeners
            .iter()
            .map(|(address, _)| *address)
            .collect();
        // One line with every affected address, rather than one `warn` per failure that nobody
        // correlates — and the same fact in the tray, so "healthy" stops being the whole story.
        tracing::warn!(
            bound = %join_addresses(&bound),
            unbound = %join_addresses(&bindings.unbound),
            "Click'n'Load did not get every configured address; another listener holds the rest"
        );
        if let Some(sink) = notice.as_ref() {
            sink(AgentNotice::PartialBind {
                bound,
                unbound: bindings.unbound.clone(),
            });
        }
    }
    // The listeners are up: the proof an updater of the agent's own waits for (RD-1210-03).
    self_update::started();
    // After a self-update the agent continues as the new program (RD-190-07); nothing to watch
    // when the system does not say which file this process runs from.
    let program = std::env::current_exe().ok();
    let replacement = program
        .clone()
        .map(|program| tokio::spawn(relaunch::watch(program, cancellation.clone())));
    let mut servers = tokio::task::JoinSet::new();
    for (address, listener) in bindings.listeners {
        servers.spawn(cnl::serve(
            address,
            listener,
            client.clone(),
            cancellation.clone(),
        ));
    }
    while let Some(result) = servers.join_next().await {
        if let Err(error) = result
            .context("Click'n'Load task failed")
            .and_then(|inner| inner)
        {
            // Cancel *before* returning. The tray takes this `Err` and ends the process in
            // `std::process::exit`, which runs no destructor and drains nothing — so a request
            // in flight used to be cut off mid-way. The ordinary signal shutdown always did
            // this; only the error path did not (RD-109-07).
            cancellation.cancel();
            wind_down(&mut background, &mut servers).await;
            return Err(error);
        }
    }
    wind_down(&mut background, &mut servers).await;
    // The listeners are closed by now, so the new program gets the Click'n'Load port.
    cancellation.cancel();
    let replaced = match replacement {
        Some(watch) => watch.await.unwrap_or(false),
        None => false,
    };
    match program {
        Some(program) if replaced => Err(anyhow::Error::new(relaunch::Replaced { program })),
        _ => Ok(()),
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => { let _ = result; }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod cli_tests;
