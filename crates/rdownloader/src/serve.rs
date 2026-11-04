//! `rdownloader serve`: the start of the service in named steps, the stop, and the deadline
//! that bounds the stop.

use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use rd_api::AppState;
use rd_scheduler::{SchedulerConfig, SchedulerHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    ServeArgs, Telemetry, load_stored_settings, plugin_boot,
    serve_stop::{ControlFile, end_after_stop_deadline, remove_control_file, shutdown_on_signal},
    startup, updater_cli,
};

/// How long the stop waits for the log sink to store the last lines, once everything else
/// has stopped.
const LOG_SINK_FINISH: Duration = Duration::from_secs(5);

/// Runs the service until it is stopped: the store, the settings, the plugins, the queue and
/// the post-processing, then the HTTP surface; the stop in the reverse order. Answers the exit
/// code a restart left to a supervisor ends with (RD-1240-32), `None` for an ordinary stop.
pub(crate) async fn run(args: ServeArgs, telemetry: Telemetry) -> Result<Option<i32>> {
    // An update the journal records ends before the database opens (RD-180-02): taken back when
    // it was interrupted, and when the files are the previous version again but this process is
    // the newer one, that previous program starts in its place.
    if updater_cli::recover(&args.paths.database)? {
        return Ok(None);
    }
    let startup::Store {
        data_directory,
        database,
        secrets,
        restore,
        // Named, so it lives to the end of `serve` rather than being dropped right here.
        instance: _instance,
        log_sink,
    } = startup::open_store(&args.paths, telemetry).await?;
    let outcome = run_on_store(&args, data_directory, database, secrets, restore).await;
    if let Err(error) = &outcome {
        tracing::error!(error = %format!("{error:#}"), "the service ended with an error");
    }
    // Last, so the lines of the stop -- and of a start that failed -- reach the store too.
    if !log_sink.finish(LOG_SINK_FINISH).await {
        tracing::warn!("the log sink did not finish in time; the last lines are on stderr only");
    }
    outcome
}

/// Everything of [`run`] after the store opened.
async fn run_on_store(
    args: &ServeArgs,
    data_directory: std::path::PathBuf,
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
    restore: rd_backup::restore::cutover::Cutover,
) -> Result<Option<i32>> {
    let (stored, runtime) = load_stored_settings(&database, &args.paths.database).await?;
    let listen = listen_address(args.listen, &stored);
    let controls = queue_controls();
    let scheduler_config = scheduler_config_for(args, &runtime, listen, &controls);
    let plugins = plugin_boot::open_plugins(args, &database, &stored, &data_directory).await?;
    let mut native = startup::native_runners(
        &database,
        &secrets,
        &scheduler_config,
        &controls.bandwidth,
        &data_directory,
        &plugins,
        args.plugin_allow_local_targets,
    )
    .await?;
    // The runners go to the scheduler; the rest of `native` feeds the flusher and the state.
    let runners = std::mem::take(&mut native.runners);
    let scheduler = start_scheduler(
        &database,
        scheduler_config,
        &secrets,
        &native.plugin_registry,
        runners,
        runtime,
    )
    .await?;
    // Written in batches, never per article (RD-1100-05); the last batch after the scheduler
    // stopped, so nothing the runner counted is left in memory.
    let traffic_flusher = native
        .usenet_traffic
        .start_flushing(database.clone(), rd_usenet::FLUSH_INTERVAL);
    let control_file = ControlFile::default();
    let shutdown = shutdown_on_signal(Arc::downgrade(&control_file));

    let (state, extraction, torrent_service) = assemble_state(
        native,
        controls,
        &scheduler,
        database,
        secrets,
        &data_directory,
        plugins,
    )
    .await;
    let running = Running {
        scheduler,
        extraction,
        torrent_service,
        traffic_flusher,
    };
    serve_until_stopped(
        state,
        args,
        &data_directory,
        &restore,
        listen,
        shutdown,
        running,
        &control_file,
    )
    .await
}

/// The stored port only applies when neither --listen nor RDOWNLOADER_LISTEN is given, so
/// an operator can always override a setting that locked them out.
fn listen_address(listen: Option<SocketAddr>, stored: &crate::StoredSettings) -> SocketAddr {
    listen.unwrap_or_else(|| {
        SocketAddr::from((
            [127, 0, 0, 1],
            stored.ui_port.unwrap_or(DEFAULT_LISTEN_PORT),
        ))
    })
}

/// The handles the queue and the post-processing are steered by, made before anything that
/// shares them.
struct QueueControls {
    postprocess_hold: rd_core::PostprocessHold,
    bandwidth: rd_scheduler::BandwidthService,
    quiet_hold: rd_core::PostprocessHold,
    power: rd_power::PowerService,
}

fn queue_controls() -> QueueControls {
    QueueControls {
        postprocess_hold: rd_core::PostprocessHold::new(),
        // Created here rather than inside the scheduler: the torrent engine owns its own
        // sockets and needs the same handle, and it is built before the scheduler is.
        bandwidth: rd_scheduler::BandwidthService::new(),
        // Raised while quiet hours defer the resource-intensive post-processing steps.
        quiet_hold: rd_core::PostprocessHold::new(),
        power: rd_power::PowerService::default(),
    }
}

/// The scheduler's configuration from the paths, the stored runtime settings and the
/// address this service answers on.
fn scheduler_config_for(
    args: &ServeArgs,
    runtime: &rd_scheduler::RuntimeSettings,
    listen: SocketAddr,
    controls: &QueueControls,
) -> SchedulerConfig {
    let mut scheduler_config = SchedulerConfig::for_directory(args.paths.downloads.clone());
    scheduler_config.postprocess_hold = controls.postprocess_hold.clone();
    scheduler_config.bandwidth = controls.bandwidth.clone();
    scheduler_config.max_active_files = runtime.max_active_files;
    scheduler_config.max_chunks_per_file = runtime.max_chunks_per_file;
    scheduler_config.max_connections_per_host = runtime.max_connections_per_host;
    scheduler_config.external_parallel_files = runtime.external_parallel_files;
    scheduler_config.speed_limit_bytes_per_second = runtime.speed_limit_bytes_per_second;
    scheduler_config.own_address = Some(listen);
    scheduler_config
}

/// Starts the queue with every runner and hands it the stored runtime settings.
async fn start_scheduler(
    database: &rd_db::Database,
    scheduler_config: SchedulerConfig,
    secrets: &rd_secrets::SecretStore,
    plugin_registry: &rd_plugin_host::PluginTypeRegistry,
    runners: Vec<Arc<dyn rd_scheduler::ExternalRunner>>,
    runtime: rd_scheduler::RuntimeSettings,
) -> Result<SchedulerHandle> {
    let scheduler = SchedulerHandle::start(
        database.clone(),
        scheduler_config,
        secrets.clone(),
        Some(plugin_registry),
        runners,
    )
    .await?;
    scheduler.update_runtime_settings(runtime).await?;
    Ok(scheduler)
}

/// Compiles the plugin extensions, adds the site rules, resumes what a stop interrupted and
/// assembles the application state; hands back the two services the stop needs besides it.
async fn assemble_state(
    native: startup::NativeRunners,
    controls: QueueControls,
    scheduler: &SchedulerHandle,
    database: rd_db::Database,
    secrets: rd_secrets::SecretStore,
    data_directory: &Path,
    plugins: rd_plugin_host::PluginInstaller,
) -> (
    AppState,
    rd_extract::ExtractionService,
    rd_torrent::TorrentService,
) {
    let startup::PluginServices {
        plugin_steps,
        storage_destinations,
        extraction,
        intake_parsers,
        crawlers,
        auth_providers,
        oauth_providers,
    } = startup::plugin_services(
        &native.plugin_registry,
        scheduler,
        &database,
        data_directory,
        controls.postprocess_hold,
        controls.quiet_hold.clone(),
        native.remote.object_storage.clone(),
    );
    // The registry holds every installed component's bytes; nothing below needs them, and a
    // local in `serve` would otherwise keep them for the life of the process.
    drop(native.plugin_registry);
    if crawlers.has_plugins() {
        tracing::info!("folder crawler plugins loaded");
    }
    // And the site rules take their place in that same selection (RD-110-06), between the
    // crawlers that name a service and the generic ones. Since RD-130-07 every rule comes
    // from the database -- the signed release file is imported, not compiled in -- so a fresh
    // installation starts with none; the adapters the rules run on live in `rd-plugin-host`,
    // where the proxies and the captcha broker already are.
    let crawlers = startup::with_site_rules(crawlers, &database, &secrets, scheduler).await;
    resume_interrupted(&extraction, &native.torrent_service).await;
    sweep_abandoned_uploads(native.remote.object_storage.clone());
    let state = AppState::new(
        database,
        scheduler.clone(),
        secrets,
        plugins,
        extraction.clone(),
        native.media_settings,
        native.media_probe,
        native.gallery_settings,
        native.stream_settings,
        native.torrent_service.clone(),
        native.torrent_settings,
        controls.power,
        controls.quiet_hold,
        native.remote,
    )
    .with_plugin_transfer_schemes(native.plugin_transfer_schemes)
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
    (state, extraction, native.torrent_service)
}

/// What runs beside the HTTP surface and is stopped after it.
struct Running {
    scheduler: SchedulerHandle,
    extraction: rd_extract::ExtractionService,
    torrent_service: rd_torrent::TorrentService,
    traffic_flusher: rd_usenet::TrafficFlusher,
}

/// Serves until the stop, then stops everything in order; the control file goes last. Answers
/// the exit code of a restart the supervisor carries out, if one was asked for. A start that
/// fails once the services run stops them in the same order before it reports the error.
#[expect(clippy::too_many_arguments)]
async fn serve_until_stopped(
    state: AppState,
    args: &ServeArgs,
    data_directory: &Path,
    restore: &rd_backup::restore::cutover::Cutover,
    listen: SocketAddr,
    shutdown: CancellationToken,
    running: Running,
    control_file: &ControlFile,
) -> Result<Option<i32>> {
    // Written only now, with everything the stop route needs in place; removed as the last
    // step of this function, so `rdownloader stop --wait` sees it gone once the queue is safe.
    let (local_control, guard) = rd_api::local_control::LocalControl::issue(data_directory, listen)
        .context("write the local control file")?;
    if let Ok(mut file) = control_file.lock() {
        *file = Some(guard);
    }
    end_after_stop_deadline(shutdown.clone(), Arc::downgrade(control_file));
    let state = state
        .with_local_control(local_control)
        .with_shutdown(shutdown.clone());
    // The services with loops of their own, stopped once the HTTP surface is gone (a request
    // may need them until then) and before the queue: the power supervisor among them, so a
    // queue the stop empties is not read as finished.
    let stop_before_queue = {
        let stream_monitor = state.stream_monitor.clone();
        let power_supervisor = state.power_supervisor.clone();
        let automations = state.automations.clone();
        let subscriptions = state.subscriptions.clone();
        let auth_flows = state.auth_flows.clone();
        let hotfolders = state.hotfolders.clone();
        let link_check = state.link_check.clone();
        let remote_jobs = state.remote_jobs.clone();
        async move {
            stream_monitor.shutdown();
            power_supervisor.shutdown();
            automations.shutdown();
            subscriptions.shutdown();
            auth_flows.shutdown();
            hotfolders.shutdown().await;
            link_check.shutdown();
            remote_jobs.shutdown();
        }
    };
    let notifications = state.notifications.clone();
    let restart = state.restart.clone();
    let result = match startup::prepare_state(&state).await {
        Ok(()) => {
            startup::finish_restore(&args.paths, restore);
            updater_cli::confirm_when_answering(&args.paths.database, listen);
            rd_api::serve(state, listen).await
        }
        Err(error) => {
            // The start ends here; what already runs stops in the usual order below.
            shutdown.cancel();
            drop(state);
            Err(error)
        }
    };
    stop_before_queue.await;
    running.torrent_service.shutdown();
    running.extraction.shutdown().await;
    let stopped = running.scheduler.shutdown().await;
    // After the queue, so what its stop reported is still queued for delivery.
    notifications.shutdown();
    running.traffic_flusher.shutdown().await;
    remove_control_file(control_file);
    stopped?;
    result?;
    Ok(restart.exit_code())
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

/// Default address when neither `--listen` nor the `ui_port` setting says otherwise.
const DEFAULT_LISTEN_PORT: u16 = 8710;
