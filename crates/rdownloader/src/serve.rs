//! `rdownloader serve`: the start of the service in named steps, the stop, and the deadline
//! that bounds the stop.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use rd_api::AppState;
use rd_scheduler::{SchedulerConfig, SchedulerHandle};
use tokio_util::sync::CancellationToken;

use crate::{ServeArgs, Telemetry, load_stored_settings, plugin_boot, startup, updater_cli};

/// Runs the service until it is stopped: the store, the settings, the plugins, the queue and
/// the post-processing, then the HTTP surface; the stop in the reverse order.
pub(crate) async fn run(args: ServeArgs, telemetry: Telemetry) -> Result<()> {
    // An update the journal records ends before the database opens (RD-180-02): taken back when
    // it was interrupted, and when the files are the previous version again but this process is
    // the newer one, that previous program starts in its place.
    if updater_cli::recover(&args.paths.database)? {
        return Ok(());
    }
    let startup::Store {
        data_directory,
        database,
        secrets,
        restore,
        // Named, so it lives to the end of `serve` rather than being dropped right here.
        instance: _instance,
    } = startup::open_store(&args.paths, telemetry).await?;
    let (stored, runtime) = load_stored_settings(&database, &args.paths.database).await?;
    // The stored port only applies when neither --listen nor RDOWNLOADER_LISTEN is given, so
    // an operator can always override a setting that locked them out.
    let listen = args.listen.unwrap_or_else(|| {
        SocketAddr::from((
            [127, 0, 0, 1],
            stored.ui_port.unwrap_or(DEFAULT_LISTEN_PORT),
        ))
    });
    let postprocess_hold = rd_core::PostprocessHold::new();
    // Created here rather than inside the scheduler: the torrent engine owns its own
    // sockets and needs the same handle, and it is built before the scheduler is.
    let bandwidth = rd_scheduler::BandwidthService::new();
    // Raised while quiet hours defer the resource-intensive post-processing steps.
    let quiet_hold = rd_core::PostprocessHold::new();
    let power = rd_power::PowerService::default();
    let mut scheduler_config = SchedulerConfig::for_directory(args.paths.downloads.clone());
    scheduler_config.postprocess_hold = postprocess_hold.clone();
    scheduler_config.bandwidth = bandwidth.clone();
    scheduler_config.max_active_files = runtime.max_active_files;
    scheduler_config.max_chunks_per_file = runtime.max_chunks_per_file;
    scheduler_config.max_connections_per_host = runtime.max_connections_per_host;
    scheduler_config.external_parallel_files = runtime.external_parallel_files;
    scheduler_config.speed_limit_bytes_per_second = runtime.speed_limit_bytes_per_second;
    scheduler_config.own_address = Some(listen);
    let plugins = plugin_boot::open_plugins(&args, &database, &stored).await?;
    let startup::NativeRunners {
        runners,
        media_settings,
        media_probe,
        gallery_settings,
        stream_settings,
        torrent_service,
        torrent_settings,
        remote,
        plugin_transfer_schemes,
        plugin_registry,
        usenet_traffic,
    } = startup::native_runners(
        &database,
        &secrets,
        &scheduler_config,
        &bandwidth,
        &data_directory,
        &plugins,
        args.plugin_allow_local_targets,
    )
    .await?;
    let scheduler = SchedulerHandle::start(
        database.clone(),
        scheduler_config,
        secrets.clone(),
        Some(&plugin_registry),
        runners,
    )
    .await?;
    scheduler.update_runtime_settings(runtime).await?;
    // Written in batches, never per article (RD-1100-05); the last batch after the scheduler
    // stopped, so nothing the runner counted is left in memory.
    let traffic_flusher =
        usenet_traffic.start_flushing(database.clone(), rd_usenet::FLUSH_INTERVAL);
    let shutdown = shutdown_on_signal();

    let startup::PluginServices {
        plugin_steps,
        storage_destinations,
        extraction,
        intake_parsers,
        crawlers,
        auth_providers,
        oauth_providers,
    } = startup::plugin_services(
        &plugin_registry,
        &scheduler,
        &database,
        &data_directory,
        postprocess_hold,
        quiet_hold.clone(),
        remote.object_storage.clone(),
    );
    // The registry holds every installed component's bytes; nothing below needs them, and a
    // local in `serve` would otherwise keep them for the life of the process.
    drop(plugin_registry);
    if crawlers.has_plugins() {
        tracing::info!("folder crawler plugins loaded");
    }
    // And the site rules take their place in that same selection (RD-110-06), between the
    // crawlers that name a service and the generic ones. Since RD-130-07 every rule comes
    // from the database -- the signed release file is imported, not compiled in -- so a fresh
    // installation starts with none; the adapters the rules run on live in `rd-plugin-host`,
    // where the proxies and the captcha broker already are.
    let crawlers = startup::with_site_rules(crawlers, &database, &secrets, &scheduler).await;
    resume_interrupted(&extraction, &torrent_service).await;
    sweep_abandoned_uploads(remote.object_storage.clone());
    let state = AppState::new(
        database,
        scheduler.clone(),
        secrets,
        plugins,
        extraction.clone(),
        media_settings,
        media_probe,
        gallery_settings,
        stream_settings,
        torrent_service.clone(),
        torrent_settings,
        power,
        quiet_hold,
        remote,
    )
    .with_plugin_transfer_schemes(plugin_transfer_schemes)
    .with_intake_parsers(intake_parsers)
    .with_crawlers(crawlers)
    .with_plugin_steps(plugin_steps)
    .with_storage_destinations(storage_destinations)
    .with_auth_providers(auth_providers, oauth_providers)
    // Compiled in by build.rs from the values VERSION.txt carries (RD-130-12).
    .with_build_info(rd_api::BuildInfo::new(
        env!("RD_BUILD_COMMIT"),
        env!("RD_BUILD_TIME"),
    ));
    // Written only now, with everything the stop route needs in place; removed as the last
    // step of this function, so `rdownloader stop --wait` sees it gone once the queue is safe.
    let (local_control, control_file) =
        rd_api::local_control::LocalControl::issue(&data_directory, listen)
            .context("write the local control file")?;
    let control_file = Arc::new(Mutex::new(Some(control_file)));
    end_after_stop_deadline(shutdown.clone(), Arc::downgrade(&control_file));
    let state = state
        .with_local_control(local_control)
        .with_shutdown(shutdown);
    startup::prepare_state(&state).await?;
    startup::finish_restore(&args.paths, &restore);
    let hotfolders = state.hotfolders.clone();
    let state_link_check = state.link_check.clone();
    let remote_jobs = state.remote_jobs.clone();
    let stream_monitor = state.stream_monitor.clone();
    updater_cli::confirm_when_answering(&args.paths.database, listen);
    let result = rd_api::serve(state, listen).await;
    stream_monitor.shutdown();
    torrent_service.shutdown();
    hotfolders.shutdown().await;
    state_link_check.shutdown();
    remote_jobs.shutdown();
    extraction.shutdown().await;
    let stopped = scheduler.shutdown().await;
    traffic_flusher.shutdown().await;
    remove_control_file(&control_file);
    stopped?;
    result
}

/// Cancelled by a signal, and by `POST /api/v1/system/shutdown` (RD-180-02): the one way to
/// stop the service gracefully on Windows, where a console-less process gets no Ctrl-C.
fn shutdown_on_signal() -> CancellationToken {
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal.cancel();
    });
    shutdown
}

/// Resumes the post-processing and the seeding a stop interrupted.
async fn resume_interrupted(
    extraction: &rd_extract::ExtractionService,
    torrent_service: &rd_torrent::TorrentService,
) {
    if let Err(error) = extraction.recover().await {
        tracing::warn!(%error, "could not resume interrupted extractions");
    }
    if let Err(error) = torrent_service.recover().await {
        tracing::warn!(%error, "could not resume seeding torrents");
    }
}

/// Multipart uploads nobody continued within a week are aborted rather than left in the
/// bucket as billed parts nothing lists (RD-150-04). In the background: it talks to the
/// services, and a slow endpoint must not delay the start.
fn sweep_abandoned_uploads(sweeper: rd_object_storage::ObjectStorageService) {
    tokio::spawn(async move {
        match sweeper
            .sweep_stale_uploads(rd_object_storage::STALE_UPLOAD_AGE)
            .await
        {
            Ok(0) => {}
            Ok(aborted) => tracing::info!(aborted, "aborted abandoned object storage uploads"),
            Err(error) => tracing::warn!(%error, "abandoned object storage uploads not swept"),
        }
    });
}

/// How long a stop may take, from the request to the end of the process (RD-180-02).
///
/// The listener drains for at most `rd_api::CONNECTION_DRAIN`, the scheduler gives its transfers
/// ten seconds to checkpoint, the rest stops in moments; this bounds all of it together and what
/// nothing else bounds -- a blocking task the runtime waits for on its way out, a stop that hangs.
/// Below the updater's wait for the stop (120 s), so a slow stop no longer fails an update. What
/// a forced end leaves is what a crash leaves, and every persistence path survives a crash
/// (`crates/rd-core/recovery-matrix.md`).
const STOP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// The local control file, removed as the last step of a stop: its removal is what
/// `rdownloader stop --wait` and the updater read as the end of the process.
type ControlFile = Arc<Mutex<Option<rd_api::local_control::ControlFileGuard>>>;

fn remove_control_file(control_file: &ControlFile) {
    if let Ok(mut file) = control_file.lock() {
        file.take();
    }
}

/// Ends the process [`STOP_DEADLINE`] after `shutdown` was cancelled, if it is still running.
///
/// On a thread of its own, so a runtime whose workers are all stuck still ends. The control file
/// goes first, as at the end of an ordinary stop, so whoever asked for the stop sees it done;
/// held weakly, so a start that fails drops it as before.
fn end_after_stop_deadline(
    shutdown: CancellationToken,
    control_file: std::sync::Weak<Mutex<Option<rd_api::local_control::ControlFileGuard>>>,
) {
    tokio::spawn(async move {
        shutdown.cancelled().await;
        std::thread::spawn(move || {
            std::thread::sleep(STOP_DEADLINE);
            tracing::error!(
                seconds = STOP_DEADLINE.as_secs(),
                "the stop did not finish in time; the process ends without the rest of it"
            );
            if let Some(control_file) = control_file.upgrade() {
                remove_control_file(&control_file);
            }
            std::process::exit(1);
        });
    });
}

/// Default address when neither `--listen` nor the `ui_port` setting says otherwise.
const DEFAULT_LISTEN_PORT: u16 = 8710;

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
    // Ctrl-C, the console window closed, and the system shutting down (RD-180-02). Windows ends
    // the process a few seconds after a close or shutdown event whatever it does, so this is
    // the best effort; `rdownloader stop` is the stop that waits for the queue.
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_close, ctrl_shutdown};

        let (Ok(mut close), Ok(mut system)) = (ctrl_close(), ctrl_shutdown()) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => { let _ = result; }
            _ = close.recv() => {}
            _ = system.recv() => {}
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
