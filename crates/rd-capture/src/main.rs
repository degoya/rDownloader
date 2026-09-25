//! Cross-platform desktop capture bridge for CNL, clipboard and NZB files.

mod activity;
mod cli;
mod client;
mod clipboard;
mod cnl;
mod commands;
mod config;
// The badge the tray draws over its icon is plain arithmetic on an RGBA buffer, so it carries the
// tray's gate plus `test` and is measured here; only the decoding and `Icon::from_rgba` around it
// need the platform crates.
#[cfg(any(windows, target_os = "macos", test))]
mod icon;
mod notify;
#[cfg(test)]
mod notify_resume;
mod os_integration;
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
        Err(error) => std::process::ExitCode::from(report(&error)),
    }
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
    }
}

/// Runs the agent, or reports that there is nothing to connect to yet.
fn guarded_run(args: RunArgs) -> Result<()> {
    if !config::is_paired(args.connection.token.as_deref()) {
        return Err(anyhow::Error::new(NotPaired));
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
    let client = CaptureClient::new(connection.service, connection.token)?;
    let signal = cancellation.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal.cancel();
    });
    let notice = desktop.as_ref().map(|desktop| desktop.notice.clone());
    // Every background task is held rather than spawned and forgotten, so its end is read
    // (RD-109-07) and so the error path below can give it a moment to stop.
    let mut background = tokio::task::JoinSet::new();
    if args.clipboard {
        tracing::info!("clipboard monitoring enabled");
        background.spawn(supervised(
            "clipboard monitoring",
            cancellation.clone(),
            notice.clone(),
            clipboard::watch_clipboard(client.clone(), cancellation.clone()),
        ));
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
    if let Some(desktop) = desktop {
        let (task_client, task_cancellation) = (client.clone(), cancellation.clone());
        background.spawn(supervised(
            "the transfer poll",
            cancellation.clone(),
            notice.clone(),
            async move {
                watch_activity(task_client, task_cancellation, desktop.activity).await;
                Ok(())
            },
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
    Ok(())
}

/// Polls the service's figures and reports what the tray should show.
///
/// The interval is `config::STATUS_POLL_INTERVAL`, the same constant the tray's health poll
/// reads, so the two states move together and the icon never contradicts the status line. The
/// rate comes with the summary; nothing here measures across a poll interval any more, so an
/// outage cannot turn into a leap.
async fn watch_activity(
    client: CaptureClient,
    cancellation: CancellationToken,
    sink: ActivitySink,
) {
    loop {
        match client.summary().await {
            Ok(summary) => sink(activity::describe(summary)),
            // `warn`, not `debug`: since RD-109-10 a byte count the service sends in a shape
            // this build cannot read fails here instead of quietly becoming zero, and a broken
            // API contract is worth a line somebody sees. The tray keeps its last figures.
            Err(error) => tracing::warn!(%error, "could not read the transfer summary"),
        }
        tokio::select! {
            () = cancellation.cancelled() => return,
            () = tokio::time::sleep(config::STATUS_POLL_INTERVAL) => {}
        }
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
mod cli_tests {
    use super::{NotPaired, PortBusy, config, report};

    #[test]
    fn an_unpaired_agent_reports_its_own_exit_code() {
        let error = anyhow::Error::new(NotPaired);
        assert_eq!(report(&error), config::EXIT_NOT_PAIRED);
    }

    #[test]
    fn a_taken_click_n_load_port_reports_its_own_exit_code() {
        let error = anyhow::Error::new(PortBusy {
            addresses: vec!["127.0.0.1:9666".parse().expect("valid address")],
        });
        assert_eq!(report(&error), config::EXIT_PORT_BUSY);
    }

    #[test]
    fn the_two_ordinary_states_do_not_share_a_code_with_a_real_failure() {
        // The launchers tell these apart by number alone, so a collision would silently turn
        // "already running" back into "could not be started".
        assert_ne!(config::EXIT_NOT_PAIRED, config::EXIT_PORT_BUSY);
        assert_eq!(report(&anyhow::anyhow!("disk on fire")), 1);
        assert_ne!(config::EXIT_NOT_PAIRED, 1);
        assert_ne!(config::EXIT_PORT_BUSY, 1);
    }

    #[test]
    fn a_taken_port_is_still_recognised_through_added_context() {
        // `run` is called through layers that add context; the marker has to survive that,
        // otherwise the code silently degrades to a plain failure.
        let error = anyhow::Error::new(PortBusy {
            addresses: vec!["127.0.0.1:9666".parse().expect("valid address")],
        })
        .context("start the capture agent");
        assert_eq!(report(&error), config::EXIT_PORT_BUSY);
    }

    #[test]
    fn a_taken_port_names_the_addresses_it_tried() {
        let busy = PortBusy {
            addresses: vec![
                "127.0.0.1:9666".parse().expect("valid address"),
                "[::1]:9666".parse().expect("valid address"),
            ],
        };
        assert_eq!(
            busy.to_string(),
            "Click'n'Load could not bind any of 127.0.0.1:9666, [::1]:9666"
        );
    }
}
