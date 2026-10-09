//! rDownloader service and diagnostic command line.

#![warn(unreachable_pub)]

use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use rd_db::Database;
use rd_scheduler::SchedulerConfig;
use tracing_subscriber::EnvFilter;

mod auth_cli;
mod doctor_site_rules;
mod instance_lock;
mod plugin_boot;
mod plugin_cli;
mod remote;
mod reset_password_cli;
mod serve;
mod site_rules_cli;
mod startup;
mod stop_cli;
mod trusted_keys;
mod updater_cli;

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
    #[command(subcommand)]
    Tools(rd_pack::tools_manifest::ToolsCommand),
    /// Builds and verifies the signed application update manifest.
    #[command(subcommand)]
    Update(rd_pack::update_manifest::UpdateCommand),
    /// Installs or removes per-user service autostart.
    Autostart(IntegrationArgs),
    /// Lists and controls the download queue of a local or remote server.
    Queue(remote::QueueArgs),
    /// Hands links to the LinkGrabber of a local or remote server and reviews them.
    Links(remote::LinksArgs),
    /// Stops the service running on this machine gracefully and waits until it has ended.
    Stop(stop_cli::StopArgs),
    /// Sign-in steps only this machine may take: `auth password-login on` switches the
    /// password sign-in back on after it was switched off for the identity provider, `auth
    /// reset-password` sets a new administrator password without the current one.
    Auth(auth_cli::AuthArgs),
    /// Installs a downloaded update and takes it back when the new version does not answer
    /// (RD-180-02). Started by the service from a copy of itself, not by hand.
    #[command(name = "apply-update", hide = true)]
    ApplyUpdate(updater_cli::ApplyArgs),
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
    /// Directory with bundled `.rdplug` packages; installed plugins are updated from it
    /// automatically when newer (default: `plugins/` next to the executable).
    #[arg(long, env = "RDOWNLOADER_BUNDLED_PLUGINS")]
    bundled_plugins: Option<PathBuf>,
    /// Installs every bundled package instead of only the chosen ones (RD-160-05).
    #[arg(long, env = "RDOWNLOADER_INSTALL_ALL_BUNDLED_PLUGINS")]
    install_all_bundled_plugins: bool,
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
        Command::Serve(args) => {
            enter_installed_home()?;
            serve::run(args, telemetry).await
        }
        Command::Doctor(args) => {
            enter_installed_home()?;
            doctor(args).await
        }
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
        Command::Tools(command) => rd_pack::tools_manifest::run(command).await,
        Command::Update(command) => rd_pack::update_manifest::run(command).await,
        Command::Autostart(args) => autostart(args),
        // Remote commands end the process themselves so a script can branch on why they
        // failed rather than on a single catch-all exit code.
        Command::Queue(args) => remote::finish(remote::queue(args).await),
        Command::Links(args) => remote::finish(remote::links(args).await),
        Command::Stop(args) => {
            // The control file lives in the data folder, which an installed build keeps in the
            // user's folder: the Start menu's "Stop rDownloader" runs from the program folder.
            enter_installed_home()?;
            remote::finish(stop_cli::run(args).await)
        }
        Command::Auth(args) => {
            // The same data folder `stop` reads its control file from.
            enter_installed_home()?;
            remote::finish(auth_cli::run(args).await)
        }
        Command::ApplyUpdate(args) => updater_cli::run(args).await,
    }
}

/// Moves an installed build into the user's data folder (RD-180-05), so the relative defaults
/// (`data/`, `downloads`) resolve there as they resolve beside the executable in the portable
/// package, whose launcher starts it in its own folder. Only for the commands that open the
/// data (`serve`, `doctor`, `stop`, `auth`): every other one keeps the working folder its relative
/// arguments were written against.
fn enter_installed_home() -> Result<()> {
    let executable = std::env::current_exe().context("locate rDownloader executable")?;
    if let Some(home) = rd_autostart::installed_home(&executable)? {
        std::fs::create_dir_all(&home).with_context(|| format!("create {}", home.display()))?;
        std::env::set_current_dir(&home).with_context(|| format!("enter {}", home.display()))?;
    }
    Ok(())
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

/// The stored settings and the runtime settings a start runs with (owner, 2026-10-04,
/// RA-DB-02): only the runtime slice (`rd_api::RUNTIME_FIELDS`) refuses the start, naming the
/// field and how to remove it; any other field that does not parse reads as its default with a
/// warning naming it, so a hand edit or a removed enum variant never locks the service out.
async fn load_stored_settings(
    database: &Database,
    path: &std::path::Path,
) -> Result<(StoredSettings, rd_scheduler::RuntimeSettings)> {
    rd_api::startup_settings(database)
        .await
        .with_context(|| format!("read the stored settings of {}", path.display()))
}

/// The settings document the service and the settings view share (audit 1.9.1, INTAKE-06):
/// the binary kept a mirror of its own with its own defaults, and the two drifted.
pub(crate) type StoredSettings = rd_api::SettingsResponse;

async fn doctor(args: DoctorArgs) -> Result<()> {
    // The self-test is its own command under the same path: it ends the process with what it
    // found, so a release preparation can branch on a rule that no longer works (RD-110-09).
    if let Some(DoctorCommand::SiteRules(check)) = &args.command {
        let code = site_rules_selftest(&args.paths, check).await?;
        std::process::exit(code);
    }
    ensure_paths(&args.paths).await?;
    let data_directory = args
        .paths
        .database
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    rd_core::set_data_directory(data_directory);
    // Read-only towards the queue: recovering interrupted jobs is the service's own start-up
    // step, and run here against a live service it reset the running downloads and steps and
    // removed packages it took for empty (audit 1.9.1, INTAKE-03).
    // Held until `doctor` returns when it was free, so no service starts and migrates beside
    // it (RA-IN-03).
    let probe = instance_lock::probe(data_directory).await;
    let database = Database::open(&args.paths.database).await?;
    if matches!(probe, instance_lock::Probe::Free(_)) {
        database.checkpoint_wal().await?;
    }
    println!("rDownloader doctor: OK");
    println!("database: {}", args.paths.database.display());
    println!(
        "downloads: {}",
        dunce::canonicalize(&args.paths.downloads)?.display()
    );
    match &probe {
        instance_lock::Probe::Held(Some(control)) => println!(
            "service: running (process {}, {})",
            control.pid, control.address
        ),
        instance_lock::Probe::Held(None) => {
            println!("service: running (starting, or not answering yet)");
        }
        instance_lock::Probe::Free(_) => println!("service: not running"),
    }
    // Field by field and never refusing: the doctor is where a start that refuses its settings
    // is explained (RA-DB-02).
    let (settings, refused) = rd_api::diagnosed_settings(&database).await?;
    if let Some(refused) = refused {
        println!("settings: a start refuses them: {refused}");
    }
    // The same checks the diagnostic bundle carries (RD-110-02), so the command and the
    // archive never disagree about what was found.
    let checks =
        rd_api::diagnostics_checks::system_checks(&rd_api::diagnostics_checks::SystemChecksInput {
            vendor_directory: settings.vendor_directory.clone(),
            trusted_proxies: settings.trusted_proxies.clone(),
            external_url: settings.external_url.clone(),
            cookie_security: settings.cookie_security,
            data_directory: rd_core::data_directory().map(std::path::Path::to_path_buf),
        })
        .await;
    print!("{}", rd_api::diagnostics_checks::render_checks(&checks));
    Ok(())
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
    let (stored, _) = load_stored_settings(&database, &paths.database).await?;
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
    // The data directory is the service account's alone from its creation on (security review
    // 2026-09-30, finding 8); Windows gets its access list in `startup::open_store`.
    if let Some(parent) = paths.database.parent()
        && !parent.as_os_str().is_empty()
    {
        rd_files::create_private_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    tokio::fs::create_dir_all(&paths.downloads)
        .await
        .with_context(|| format!("create {}", paths.downloads.display()))?;
    Ok(())
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
