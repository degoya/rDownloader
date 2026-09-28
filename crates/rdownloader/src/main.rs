//! rDownloader service and diagnostic command line.

use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use rd_api::AppState;
use rd_db::Database;
use rd_scheduler::{SchedulerConfig, SchedulerHandle};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

mod doctor_site_rules;
mod plugin_cli;
mod remote;
mod site_rules_cli;
mod startup;
mod tools_cli;
mod trusted_keys;

#[derive(Parser)]
#[command(
    name = "rdownloader",
    version,
    about = "Local-first cross-platform download manager"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Runs the REST API, web interface and download scheduler.
    Serve(ServeArgs),
    /// Validates paths, SQLite and the runtime environment.
    Doctor(DoctorArgs),
    /// Prints or writes the generated REST contract as OpenAPI JSON.
    Openapi(OpenapiArgs),
    /// Generates keys, packages, verifies and installs signed resolver packages.
    Plugin(plugin_cli::PluginArgs),
    /// Signs the manifest that drives the managed external tools.
    Tools(tools_cli::ToolsArgs),
    /// Signs and verifies the rule file that recognises release pages.
    #[command(subcommand)]
    SiteRules(rd_pack::site_rules::SiteRulesCommand),
    /// Installs or removes per-user service autostart.
    Autostart(IntegrationArgs),
    /// Lists and controls the download queue of a local or remote server.
    Queue(remote::QueueArgs),
    /// Hands links to the LinkGrabber of a local or remote server and reviews them.
    Links(remote::LinksArgs),
}

#[derive(Args)]
struct IntegrationArgs {
    #[command(subcommand)]
    command: IntegrationCommand,
}

#[derive(Subcommand)]
enum IntegrationCommand {
    /// Registers the service for the next user login.
    Install,
    /// Removes the service's per-user login registration.
    Remove,
}

#[derive(Clone, Args)]
struct CommonPaths {
    /// SQLite database file.
    #[arg(
        long,
        env = "RDOWNLOADER_DATABASE",
        default_value = "data/rdownloader.sqlite3"
    )]
    database: PathBuf,
    /// Allowlisted primary download directory.
    #[arg(long, env = "RDOWNLOADER_DOWNLOADS", default_value = "downloads")]
    downloads: PathBuf,
}

#[derive(Args)]
struct ServeArgs {
    #[command(flatten)]
    paths: CommonPaths,
    /// Loopback address used by the web interface and API. Overrides the `ui_port` setting;
    /// without either, `127.0.0.1:8710` is used.
    #[arg(long, env = "RDOWNLOADER_LISTEN")]
    listen: Option<SocketAddr>,
    /// Directory containing installed, versioned resolver packages.
    #[arg(long, env = "RDOWNLOADER_PLUGIN_ROOT", default_value = "data/plugins")]
    plugin_root: PathBuf,
    /// Trusted plugin key as KEY_ID=BASE64_ED25519_PUBLIC_KEY; repeatable.
    #[arg(long = "trusted-plugin-key")]
    trusted_plugin_keys: Vec<String>,
    /// Allows unsigned plugin installation. Never enable for production.
    #[arg(long, env = "RDOWNLOADER_PLUGIN_DEVELOPMENT_MODE")]
    plugin_development_mode: bool,
    /// Allows installed transfer backends to dial loopback and private-network addresses.
    /// Separate from --plugin-development-mode on purpose: never enable for production.
    #[arg(long, env = "RDOWNLOADER_PLUGIN_ALLOW_LOCAL_TARGETS")]
    plugin_allow_local_targets: bool,
    /// Directory with bundled `.rdplug` packages installed automatically when newer
    /// (default: `plugins/` next to the executable).
    #[arg(long, env = "RDOWNLOADER_BUNDLED_PLUGINS")]
    bundled_plugins: Option<PathBuf>,
    /// Ignores the release signing key compiled into this binary.
    #[arg(long)]
    no_default_plugin_key: bool,
}

#[derive(Args)]
struct DoctorArgs {
    #[command(flatten)]
    paths: CommonPaths,
    #[command(subcommand)]
    command: Option<DoctorCommand>,
}

#[derive(Subcommand)]
enum DoctorCommand {
    /// Asks every site rule about its own probe address and reports what it found.
    SiteRules(doctor_site_rules::SiteRulesCheckArgs),
}

#[derive(Args)]
struct OpenapiArgs {
    /// Writes JSON to this path instead of standard output.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let telemetry = init_tracing();
    match Cli::parse().command {
        Command::Serve(args) => serve(args, telemetry).await,
        Command::Doctor(args) => doctor(args).await,
        Command::Openapi(args) => {
            let document = serde_json::to_vec_pretty(&rd_api::openapi_document())?;
            if let Some(output) = args.output {
                if let Some(parent) = output.parent()
                    && !parent.as_os_str().is_empty()
                {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(output, document).await?;
            } else {
                println!("{}", String::from_utf8(document)?);
            }
            Ok(())
        }
        Command::Plugin(args) => plugin_cli::run(args).await,
        Command::Tools(args) => tools_cli::run(args).await,
        Command::SiteRules(command) => rd_pack::site_rules::run(command).await,
        Command::Autostart(args) => autostart(args),
        // Remote commands end the process themselves so a script can branch on why they
        // failed rather than on a single catch-all exit code.
        Command::Queue(args) => remote::finish(remote::queue(args).await),
        Command::Links(args) => remote::finish(remote::links(args).await),
    }
}

fn autostart(args: IntegrationArgs) -> Result<()> {
    match args.command {
        IntegrationCommand::Install => {
            let executable = std::env::current_exe().context("locate rDownloader executable")?;
            rd_autostart::install(rd_autostart::Target::Server, &executable)?;
            println!("rDownloader service autostart installed for the next login.");
        }
        IntegrationCommand::Remove => {
            rd_autostart::remove(rd_autostart::Target::Server)?;
            println!("rDownloader service autostart removed.");
        }
    }
    Ok(())
}

async fn serve(args: ServeArgs, telemetry: Telemetry) -> Result<()> {
    let startup::Store {
        data_directory,
        database,
        secrets,
    } = startup::open_store(&args.paths, telemetry).await?;
    let stored = load_stored_settings(&database).await?;
    let runtime = runtime_settings(&stored)?;
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
    let plugins = startup::open_plugins(&args, &database, &stored).await?;
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
    let shutdown = CancellationToken::new();
    let signal = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal.cancel();
    });

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
    if let Err(error) = extraction.recover().await {
        tracing::warn!(%error, "could not resume interrupted extractions");
    }
    if let Err(error) = torrent_service.recover().await {
        tracing::warn!(%error, "could not resume seeding torrents");
    }
    // Multipart uploads nobody continued within a week are aborted rather than left in the
    // bucket as billed parts nothing lists (RD-150-04). In the background: it talks to the
    // services, and a slow endpoint must not delay the start.
    let sweeper = remote.object_storage.clone();
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
    startup::prepare_state(&state).await?;
    let hotfolders = state.hotfolders.clone();
    let state_link_check = state.link_check.clone();
    let remote_jobs = state.remote_jobs.clone();
    let stream_monitor = state.stream_monitor.clone();
    let result = rd_api::serve(state, listen, shutdown).await;
    stream_monitor.shutdown();
    torrent_service.shutdown();
    hotfolders.shutdown().await;
    state_link_check.shutdown();
    remote_jobs.shutdown();
    extraction.shutdown().await;
    scheduler.shutdown().await?;
    result
}

#[derive(Deserialize)]
#[serde(default)]
struct StoredSettings {
    /// Plugin ids the user switched off; they stay installed but are never loaded.
    #[serde(default)]
    disabled_plugins: Vec<String>,
    max_active_files: u32,
    max_chunks_per_file: u32,
    /// Simultaneous connections one host may see; absent in older blobs, hence the default.
    #[serde(default = "default_connections_per_host")]
    max_connections_per_host: u32,
    /// NNTP connections one file may hold; `0` is as many as the servers allow (RD-108-25).
    nntp_connections_per_file: u32,
    /// NZB files at once; `0` is automatic (RD-130-22). Absent in older blobs, hence zero.
    nntp_parallel_files: u32,
    speed_limit_bytes_per_second: Option<rd_core::ByteCount>,
    upload_limit_bytes_per_second: Option<rd_core::ByteCount>,
    generate_sha256: bool,
    global_proxy_profile_id: Option<rd_core::ProxyProfileId>,
    custom_ca_pem: Option<String>,
    /// Address ranges whose forwarded headers are believed (RD-100-11).
    #[serde(default)]
    trusted_proxies: Vec<String>,
    /// What the outside world calls this installation.
    #[serde(default)]
    external_url: Option<String>,
    /// When the session cookie is marked `Secure`.
    #[serde(default)]
    cookie_security: rd_authn::CookieSecurity,
    max_retries: u32,
    pause_during_postprocess: bool,
    #[serde(default = "enabled")]
    torrent_service_enabled: bool,
    #[serde(default = "enabled")]
    usenet_service_enabled: bool,
    #[serde(default = "enabled")]
    media_service_enabled: bool,
    #[serde(default = "enabled")]
    gallery_service_enabled: bool,
    #[serde(default = "enabled")]
    recording_service_enabled: bool,
    #[serde(default = "enabled")]
    remote_service_enabled: bool,
    ui_port: Option<u16>,
    vendor_directory: Option<String>,
}

/// A service switch absent from an older settings blob means the service is on.
const fn enabled() -> bool {
    true
}

/// A settings blob written before the per-host limit existed gets the default rather than
/// an unbounded zero.
const fn default_connections_per_host() -> u32 {
    rd_scheduler::DEFAULT_CONNECTIONS_PER_HOST as u32
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            disabled_plugins: Vec::new(),
            max_active_files: 3,
            max_chunks_per_file: 4,
            max_connections_per_host: default_connections_per_host(),
            nntp_connections_per_file: 0,
            nntp_parallel_files: 0,
            speed_limit_bytes_per_second: None,
            upload_limit_bytes_per_second: None,
            generate_sha256: true,
            global_proxy_profile_id: None,
            custom_ca_pem: None,
            trusted_proxies: Vec::new(),
            external_url: None,
            cookie_security: rd_authn::CookieSecurity::default(),
            max_retries: rd_scheduler::DEFAULT_MAX_RETRIES,
            pause_during_postprocess: true,
            torrent_service_enabled: true,
            usenet_service_enabled: true,
            media_service_enabled: true,
            gallery_service_enabled: true,
            recording_service_enabled: true,
            remote_service_enabled: true,
            ui_port: None,
            vendor_directory: None,
        }
    }
}

/// Default address when neither `--listen` nor the `ui_port` setting says otherwise.
const DEFAULT_LISTEN_PORT: u16 = 8710;

/// Refuses a malformed blob rather than running on defaults: these are the concurrency,
/// listen-port and directory settings the whole process is built from, and starting on
/// silent defaults would contradict everything the person configured.
async fn load_stored_settings(database: &Database) -> Result<StoredSettings> {
    database.service_settings().await
}

fn runtime_settings(stored: &StoredSettings) -> Result<rd_scheduler::RuntimeSettings> {
    anyhow::ensure!(
        stored.max_active_files > 0,
        "stored max_active_files is zero"
    );
    anyhow::ensure!(
        stored.max_chunks_per_file > 0,
        "stored max_chunks_per_file is zero"
    );
    Ok(rd_scheduler::RuntimeSettings {
        max_active_files: stored.max_active_files as usize,
        max_chunks_per_file: stored.max_chunks_per_file as usize,
        max_connections_per_host: stored.max_connections_per_host as usize,
        external_connections_per_file: stored.nntp_connections_per_file as usize,
        external_parallel_files: stored.nntp_parallel_files as usize,
        speed_limit_bytes_per_second: stored
            .speed_limit_bytes_per_second
            .map(rd_core::ByteCount::get),
        upload_limit_bytes_per_second: stored
            .upload_limit_bytes_per_second
            .map(rd_core::ByteCount::get),
        generate_sha256: stored.generate_sha256,
        global_proxy_profile_id: stored.global_proxy_profile_id,
        custom_ca_pem: stored.custom_ca_pem.clone(),
        max_retries: stored
            .max_retries
            .min(rd_scheduler::MAX_CONFIGURABLE_RETRIES),
        pause_during_postprocess: stored.pause_during_postprocess,
        disabled_kinds: rd_core::ServiceSwitches {
            torrent: stored.torrent_service_enabled,
            usenet: stored.usenet_service_enabled,
            media: stored.media_service_enabled,
            gallery: stored.gallery_service_enabled,
            recording: stored.recording_service_enabled,
            remote: stored.remote_service_enabled,
        }
        .disabled_kinds(),
    })
}

async fn doctor(args: DoctorArgs) -> Result<()> {
    // The self-test is its own command under the same path: it ends the process with what it
    // found, so a release preparation can branch on a rule that no longer works (RD-110-09).
    if let Some(DoctorCommand::SiteRules(check)) = &args.command {
        let code = site_rules_selftest(&args.paths, check).await?;
        std::process::exit(code);
    }
    ensure_paths(&args.paths).await?;
    rd_core::set_data_directory(
        args.paths
            .database
            .parent()
            .unwrap_or_else(|| std::path::Path::new(".")),
    );
    let database = Database::open(&args.paths.database).await?;
    let recovered = database.recover_interrupted().await?;
    database.checkpoint_wal().await?;
    println!("rDownloader doctor: OK");
    println!("database: {}", args.paths.database.display());
    println!(
        "downloads: {}",
        dunce::canonicalize(&args.paths.downloads)?.display()
    );
    println!("recovered jobs: {recovered}");
    println!("toolchain:");
    for (tool, required) in [
        ("cargo-xwin", false),
        ("clang-cl", false),
        ("lld-link", false),
        ("ninja", false),
        ("nasm", false),
        ("node", true),
        ("npm", true),
        ("docker", false),
    ] {
        let state = if command_available(tool) {
            "OK"
        } else if required {
            "MISSING"
        } else {
            "optional/not found"
        };
        println!("  {tool}: {state}");
    }
    let settings = load_stored_settings(&database).await?;
    // The same checks the diagnostic bundle carries (RD-110-02), so the command and the
    // archive never disagree about what was found.
    let checks =
        rd_api::diagnostics_checks::system_checks(&rd_api::diagnostics_checks::SystemChecksInput {
            vendor_directory: settings.vendor_directory.clone(),
            trusted_proxies: settings.trusted_proxies.clone(),
            external_url: settings.external_url.clone(),
            cookie_security: settings.cookie_security,
        })
        .await;
    print!("{}", rd_api::diagnostics_checks::render_checks(&checks));
    if cfg!(target_os = "linux")
        && (!command_available("clang-cl") || !command_available("lld-link"))
    {
        println!("  Note: the WSL Windows build (cargo xwin) requires clang-cl and lld-link.");
    }
    Ok(())
}

fn command_available(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// One self-test run, with the adapters a running service would use: the same proxy profile
/// and the same custom CA material, read out of the settings this installation stores.
///
/// No scheduler and no captcha broker -- see `doctor_site_rules` for why the second is right
/// rather than a gap.
async fn site_rules_selftest(
    paths: &CommonPaths,
    check: &doctor_site_rules::SiteRulesCheckArgs,
) -> Result<i32> {
    ensure_paths(paths).await?;
    let data_directory = paths
        .database
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    rd_core::set_data_directory(&data_directory);
    let database = Database::open(&paths.database).await?;
    let secrets =
        rd_secrets::SecretStore::open_with_os_keyring(data_directory.join("secrets")).await?;
    database.install_secret_vault(secrets.clone());
    let stored = load_stored_settings(&database).await?;
    let network_defaults = SchedulerConfig::for_directory(paths.downloads.clone()).network_defaults;
    {
        let mut defaults = network_defaults.write().await;
        defaults.global_proxy_profile_id = stored.global_proxy_profile_id;
        defaults.custom_ca_pem = stored
            .custom_ca_pem
            .clone()
            .filter(|value| !value.trim().is_empty())
            .map(String::into_bytes)
            .into_iter()
            .collect();
    }
    let network = rd_plugin_host::RuleNetwork::new(database.clone(), secrets, network_defaults);
    doctor_site_rules::run(&database, &network, check).await
}

async fn ensure_paths(paths: &CommonPaths) -> Result<()> {
    if let Some(parent) = paths.database.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    tokio::fs::create_dir_all(&paths.downloads)
        .await
        .with_context(|| format!("create {}", paths.downloads.display()))?;
    Ok(())
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

/// Diagnostics go to stderr, always.
///
/// A command with machine-readable output has to be able to promise that stdout carries the
/// document and nothing else — `plugin conformance --json | jq` was reading a log line as the
/// first character of its JSON. Keeping logs on stderr is also what a shell pipeline expects
/// of any command, so this is not a special case for one subcommand.
///
/// The structured log store (RD-110-02) is a second layer on the same subscriber, behind the
/// same filter, so the store holds what stderr showed and nothing more. It is installed here,
/// before the database exists, because the lines worth keeping are the ones from start-up;
/// its channel buffers them until `serve` opens the database and starts the sink.
fn init_tracing() -> Telemetry {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("rdownloader=info,rd_=info"));
    let (capture, logs) = rd_diagnostics::install();
    let (sink, spans) = rd_diagnostics::otlp::channel();
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(capture)
        // The trace export (RD-110-03). Installed unconditionally and inert until the
        // settings switch it on: the layer checks one atomic per closed span while it is off,
        // and a subscriber cannot gain a layer after `init`.
        .with(rd_diagnostics::TraceExportLayer::new(sink))
        .init();
    Telemetry { logs, spans }
}

/// What `init_tracing` produced and `serve` connects to the database once it opens.
struct Telemetry {
    logs: rd_diagnostics::LogStream,
    spans: rd_diagnostics::SpanStream,
}

/// Installs newer bundled `.rdplug` packages from `<exe dir>/plugins` (or the given directory).
/// Restores the trust-on-first-use keys so installed third-party plugins still verify.
async fn load_trusted_plugin_keys(database: &Database, verifier: &rd_plugin_host::PluginVerifier) {
    let keys = match database.list_plugin_trusted_keys().await {
        Ok(keys) => keys,
        Err(error) => {
            tracing::warn!(error = %error, "could not read confirmed plugin signing keys");
            return;
        }
    };
    for key in keys {
        if let Err(error) = verifier.trust_key_base64(key.key_id.clone(), &key.public_key) {
            tracing::warn!(
                key_id = %key.key_id,
                error = %error,
                "skipping unreadable confirmed plugin signing key"
            );
        }
    }
}

/// Seeds the plugin installer with the stored version choices (RD-140-02).
///
/// A choice that cannot be read costs the choice, not the start: every plugin then runs its
/// newest version, which is what it did before choices existed, and the failure is said out
/// loud.
async fn load_plugin_version_choices(
    database: &Database,
    plugins: &rd_plugin_host::PluginInstaller,
) {
    match database.list_plugin_version_choices().await {
        Ok(rows) => plugins.set_version_choices(
            rows.into_iter()
                .map(|row| {
                    (
                        row.plugin_id,
                        rd_plugin_host::VersionChoice {
                            active: row.active_version,
                            staged: row.staged_version,
                        },
                    )
                })
                .collect(),
        ),
        Err(error) => {
            tracing::warn!(error = %error, "could not read the plugin version choices");
        }
    }
}

/// Restores the package digests an operator has withdrawn.
///
/// Read before anything loads a plugin: the whole point of a withdrawal is that the package is
/// refused on the *next* load, so a set restored afterwards would first let through exactly the
/// version it names. A row that cannot be read costs that one withdrawal and is said out loud;
/// none of this may keep the service from starting, because a database that cannot be read here
/// is a problem to report, not a reason to leave the person without a download manager.
async fn load_withdrawn_plugin_digests(
    database: &Database,
    verifier: &rd_plugin_host::PluginVerifier,
) {
    let revocations = match database.list_plugin_digest_revocations().await {
        Ok(revocations) => revocations,
        Err(error) => {
            tracing::warn!(error = %error, "could not read withdrawn plugin packages");
            return;
        }
    };
    let mut digests = Vec::with_capacity(revocations.len());
    for revocation in revocations {
        match rd_plugin_host::parse_package_digest(&revocation.digest) {
            Ok(digest) => digests.push(digest),
            Err(error) => tracing::warn!(
                digest = %revocation.digest,
                error = %error,
                "skipping an unreadable withdrawn plugin package"
            ),
        }
    }
    // One call: it replaces the set rather than adding to it.
    if let Err(error) = verifier.set_revoked_package_digests(digests) {
        tracing::warn!(error = %error, "could not apply the withdrawn plugin packages");
    }
}

/// Restores the plugin signing keys a repository index withdrew (RD-140-01).
///
/// Read before anything loads a plugin, for the same reason as the withdrawn digests; a key
/// withdrawn once stays withdrawn even when a later index no longer names it.
async fn load_withdrawn_plugin_keys(
    database: &Database,
    verifier: &rd_plugin_host::PluginVerifier,
) {
    match database.list_plugin_withdrawn_keys().await {
        Ok(keys) => {
            if let Err(error) =
                verifier.set_withdrawn_keys(keys.into_iter().map(|key| key.fingerprint))
            {
                tracing::warn!(error = %error, "could not apply the withdrawn plugin signing keys");
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "could not read withdrawn plugin signing keys");
        }
    }
}

async fn sync_bundled_plugins(
    plugins: &rd_plugin_host::PluginInstaller,
    directory: Option<PathBuf>,
) {
    let directory = directory.or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("plugins")))
    });
    let Some(directory) = directory else {
        return;
    };
    // An image built without scripts/build-plugins.sh copies an empty directory, and the result
    // is a service with no hoster resolvers and nothing anywhere saying why. A *missing*
    // directory is the ordinary case for a plain binary and stays quiet.
    if let Ok(entries) = std::fs::read_dir(&directory)
        && !entries.flatten().any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "rdplug")
        })
    {
        tracing::warn!(
            directory = %directory.display(),
            "no bundled plugin packages found; hoster resolvers and other plugin providers will be missing"
        );
    }
    match rd_plugin_host::sync_bundled(plugins, &directory).await {
        Ok(report) => {
            for (id, version) in &report.installed {
                tracing::info!(%id, version, "installed bundled plugin");
            }
            for (name, reason) in &report.rejected {
                tracing::warn!(package = name, reason, "bundled plugin rejected");
            }
        }
        Err(error) => {
            tracing::warn!(%error, directory = %directory.display(), "bundled plugin sync failed")
        }
    }
}
