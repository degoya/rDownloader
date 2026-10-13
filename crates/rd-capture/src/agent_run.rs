//! One run of the agent, in the order it happens: the connection, the background tasks, the
//! Click'n'Load listeners, and the end -- a shutdown, a failed listener or the hand-over to an
//! updated program. Split from `main.rs` into named steps (RD-1280-08); the headless path and
//! the tray both enter through [`run`].

use std::net::SocketAddr;

use anyhow::{Context, Result};
use tokio::{net::TcpListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    ActivitySink, DesktopSinks, PortBusy, activity,
    cli::RunArgs,
    client::CaptureClient,
    clipboard, cnl, config, controls, game_mode, hotkeys, notify, relaunch, self_update,
    server_update,
    supervision::{
        AgentNotice, NoticeSink, bind_click_n_load, join_addresses, supervised, wind_down,
    },
};

/// Runs the agent until `cancellation` fires, a Click'n'Load listener fails or an updated
/// program takes over.
pub(crate) async fn run(
    args: RunArgs,
    cancellation: CancellationToken,
    desktop: Option<DesktopSinks>,
) -> Result<()> {
    require_loopback(&args.cnl_listen)?;
    let connection = config::load(args.connection.service, args.connection.token)?;
    // For the Linux shortcut listener, which opens the web interface itself, and for the server
    // update's page (RD-1240-25).
    let service = connection.service.clone();
    let client = CaptureClient::new(connection.service, connection.token)?;
    cancel_on_shutdown_signal(&cancellation);
    let notice = desktop.as_ref().map(|desktop| desktop.notice.clone());
    // Every background task is held rather than spawned and forgotten, so its end is read
    // (RD-109-07) and so the error path below can give it a moment to stop.
    let mut background = JoinSet::new();
    let watchers = Watchers {
        clipboard: args.clipboard,
        notifications: !args.no_notifications,
    };
    spawn_background(
        &mut background,
        &client,
        service,
        &cancellation,
        notice.clone(),
        desktop,
        watchers,
    );
    let listeners = bind_listeners(
        args.cnl_listen,
        notice.as_ref(),
        &mut background,
        &cancellation,
    )
    .await?;
    serve(listeners, &client, &cancellation, &mut background).await
}

/// Click'n'Load is reachable from the local browser only, never from the network.
fn require_loopback(listen: &[SocketAddr]) -> Result<()> {
    if listen.is_empty() || listen.iter().any(|address| !address.ip().is_loopback()) {
        anyhow::bail!("Click'n'Load requires at least one loopback address");
    }
    Ok(())
}

/// Ends the run on Ctrl-C or, on Unix, `SIGTERM`.
fn cancel_on_shutdown_signal(cancellation: &CancellationToken) {
    let signal = cancellation.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal.cancel();
    });
}

/// The optional watchers `run` was asked for.
struct Watchers {
    clipboard: bool,
    notifications: bool,
}

/// Starts every background task of a run: the update checks, the settings poll, game mode, the
/// optional watchers and the transfer poll or, without a tray, the queue requests.
fn spawn_background(
    background: &mut JoinSet<()>,
    client: &CaptureClient,
    service: url::Url,
    cancellation: &CancellationToken,
    notice: Option<NoticeSink>,
    desktop: Option<DesktopSinks>,
    watchers: Watchers,
) {
    let offer_sink = desktop.as_ref().map(|desktop| desktop.update.clone());
    let server_update_sink = desktop
        .as_ref()
        .map(|desktop| desktop.server_update.clone());
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
        background,
        client,
        cancellation,
        offer_sink,
        update_requests,
    );
    server_update::spawn(
        background,
        client,
        service.clone(),
        cancellation,
        notice.clone(),
        server_update_sink,
        server_update_requests,
    );
    let settings = controls::follow_settings(
        background,
        client,
        cancellation,
        notice.clone(),
        settings_requests,
        desktop.as_ref().map(|(_, sink)| sink.clone()),
    );
    // Pausing for a game (RD-1240-19); it does nothing until the settings name a trigger.
    game_mode::spawn(
        background,
        client,
        cancellation,
        notice.clone(),
        settings.clone(),
    );
    if watchers.clipboard {
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
            background,
            controls,
            settings.clone(),
            service,
            cancellation,
        );
    }
    if watchers.notifications {
        spawn_intake_notifications(background, client, cancellation, notice.clone());
    } else {
        tracing::debug!("desktop notifications disabled");
    }
    spawn_transfer_poll(
        background,
        client,
        cancellation,
        notice,
        desktop.map(|(activity, _)| activity),
        queue,
    );
}

/// The desktop notifications about new intake.
fn spawn_intake_notifications(
    background: &mut JoinSet<()>,
    client: &CaptureClient,
    cancellation: &CancellationToken,
    notice: Option<NoticeSink>,
) {
    let (task_client, task_cancellation) = (client.clone(), cancellation.clone());
    background.spawn(supervised(
        "intake notifications",
        cancellation.clone(),
        notice,
        async move {
            notify::watch_intake(task_client, task_cancellation).await;
            Ok(())
        },
    ));
}

/// The transfer poll, only when something is displaying it.
///
/// The polls run on the agent's side rather than beside the health check because they need the
/// capture token, and the tray deliberately never reads the keyring itself -- a second lookup
/// risks a second macOS Keychain prompt.
fn spawn_transfer_poll(
    background: &mut JoinSet<()>,
    client: &CaptureClient,
    cancellation: &CancellationToken,
    notice: Option<NoticeSink>,
    activity: Option<ActivitySink>,
    queue: tokio::sync::mpsc::UnboundedReceiver<activity::QueueRequest>,
) {
    let (task_client, task_cancellation) = (client.clone(), cancellation.clone());
    if let Some(activity) = activity {
        background.spawn(supervised(
            "the transfer poll",
            cancellation.clone(),
            notice,
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
}

/// Binds the Click'n'Load addresses. Not one of them ends the run as [`PortBusy`]; some of them
/// is reported, in the log and to the tray.
async fn bind_listeners(
    listen: Vec<SocketAddr>,
    notice: Option<&NoticeSink>,
    background: &mut JoinSet<()>,
    cancellation: &CancellationToken,
) -> Result<Vec<(SocketAddr, TcpListener)>> {
    let attempted = listen.clone();
    let bindings = bind_click_n_load(listen).await;
    if bindings.listeners.is_empty() {
        cancellation.cancel();
        wind_down(background, &mut JoinSet::new()).await;
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
        if let Some(sink) = notice {
            sink(AgentNotice::PartialBind {
                bound,
                unbound: bindings.unbound.clone(),
            });
        }
    }
    Ok(bindings.listeners)
}

/// Serves Click'n'Load until the run ends, then continues as the updated program when one
/// replaced this one.
async fn serve(
    listeners: Vec<(SocketAddr, TcpListener)>,
    client: &CaptureClient,
    cancellation: &CancellationToken,
    background: &mut JoinSet<()>,
) -> Result<()> {
    // The listeners are up: the proof an updater of the agent's own waits for (RD-1210-03).
    self_update::started();
    // After a self-update the agent continues as the new program (RD-190-07); nothing to watch
    // when the system does not say which file this process runs from.
    let program = std::env::current_exe().ok();
    let replacement = program
        .clone()
        .map(|program| tokio::spawn(relaunch::watch(program, cancellation.clone())));
    let mut servers = JoinSet::new();
    for (address, listener) in listeners {
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
            wind_down(background, &mut servers).await;
            return Err(error);
        }
    }
    wind_down(background, &mut servers).await;
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
