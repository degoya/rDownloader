//! The parts `serve` starts the service from, in the order it starts them.
//!
//! Each function is a stretch of `serve` moved out whole: the calls, their order and their
//! arguments are the ones `serve` made inline.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_api::AppState;
use rd_backup::restore::cutover::{self, Cutover};
use rd_db::Database;
use rd_scheduler::{SchedulerConfig, SchedulerHandle};

use crate::{CommonPaths, Telemetry, doctor_site_rules, ensure_paths, site_rules_cli};

/// The data directory with the database and the vault that live in it.
pub(crate) struct Store {
    pub(crate) data_directory: PathBuf,
    pub(crate) database: Database,
    pub(crate) secrets: rd_secrets::SecretStore,
    /// What this start did with a restore waiting for it (RD-160-03); a switched one is
    /// finished once the start completed ([`finish_restore`]).
    pub(crate) restore: Cutover,
    /// This service's hold on the data directory; kept until the process ends.
    pub(crate) instance: crate::instance_lock::InstanceLock,
}

/// Opens the database and the vault, and starts the log sink and the trace exporter.
///
/// A restore staged by the running service switches here, before the database opens
/// (RD-160-03). A restored database that does not open is put back in the same start, and the
/// previous installation starts instead.
pub(crate) async fn open_store(paths: &CommonPaths, telemetry: Telemetry) -> Result<Store> {
    ensure_paths(paths).await?;
    let data_directory = paths
        .database
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_path_buf();
    rd_core::set_data_directory(&data_directory);
    // Before anything the service writes lands in it — the local control token, the journal of
    // an update (security review 2026-09-30, finding 2). A folder that keeps other accounts in
    // is a warning, not a refusal; `doctor` reports it.
    let private = if data_directory.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        data_directory.as_path()
    };
    if let Err(error) = rd_files::protect_private_dir(private) {
        tracing::warn!(%error, path = %private.display(), "the data directory could not be made private; other accounts on this machine may read the database and the local control token");
    }
    // Before a restore switches or anything recovers: a second service would take the first
    // one's running downloads for interrupted ones (audit 1.9.1, INTAKE-03).
    let instance = crate::instance_lock::acquire(&data_directory)?;
    let layout = cutover::Layout::new(&paths.database);
    let mut restore = cutover::apply_pending(&layout)?;
    let database = match Database::open(&paths.database).await {
        Ok(database) => database,
        Err(error) if matches!(restore, Cutover::Switched(_)) => {
            let reason = format!("the restored database does not open: {error:#}");
            tracing::error!(%reason, "the previous installation is put back");
            let pending = cutover::roll_back(&layout, &reason)?;
            restore = Cutover::RolledBack { pending, reason };
            Database::open(&paths.database).await?
        }
        Err(error) => return Err(error),
    };
    // Before anything compiles a plugin: the cache is part of the engine the first compile
    // builds (RD-130-06). Without it the service still starts, it only compiles every plugin.
    if let Err(error) =
        rd_plugin_host::configure_compile_cache(&data_directory.join("plugin-cache"))
    {
        tracing::warn!(%error, "plugins compile on every start: the compile cache is unavailable");
    }
    // The log sink starts as soon as there is a store to write to; the records from the
    // lines above are waiting in its channel.
    rd_diagnostics::sink::spawn(telemetry.logs, database.clone());
    // The trace exporter reads the settings on every batch, so switching the export on or
    // pointing it elsewhere takes effect without a restart. Nothing is sent until it does.
    rd_diagnostics::otlp::spawn(
        telemetry.spans,
        database.clone(),
        env!("CARGO_PKG_VERSION").to_owned(),
    );
    // A keychain that would hand the master key out only after a prompt nobody sees ends the
    // start, said in the log with the way out, instead of waiting for ever (RD-1200-02).
    let secrets =
        match rd_secrets::SecretStore::open_with_os_keyring(data_directory.join("secrets")).await {
            Ok(secrets) => secrets,
            Err(error) => {
                if let Some(refused) = error.downcast_ref::<rd_secrets::KeyringInteractionRefused>()
                {
                    tracing::error!(code = rd_secrets::KEYRING_INTERACTION_REFUSED, "{refused}");
                }
                return Err(error);
            }
        };
    // A link fragment that is key material goes into this vault at intake instead of being
    // dropped (RD-110-38). Installed rather than passed to `Database::open`, because the
    // store has to exist before the vault's master key is fetched from the keyring.
    database.install_secret_vault(secrets.clone());
    // A data folder copied from another machine or account arrives without its master key; say
    // so once at the start, counted, instead of only as failed tests later (RD-1240-36).
    if let Ok(counts) = secrets.readability().await
        && counts.unreadable > 0
    {
        tracing::warn!(
            code = rd_secrets::SECRET_UNREADABLE,
            unreadable = counts.unreadable,
            readable = counts.readable,
            "stored credentials cannot be read with this installation's master key; enter them \
             again or restore a full backup with its passphrase (`rdownloader doctor`)"
        );
    }
    // A restore that was put back leaves the credentials it put into the vault; they belong to
    // nothing now.
    if let Cutover::RolledBack {
        pending: Some(pending),
        ..
    } = &restore
    {
        for reference in &pending.minted_secrets {
            if let Err(error) = secrets.remove(reference).await {
                tracing::warn!(%error, "a restore's credential could not be removed");
            }
        }
    }
    take_over_archive_passwords(&database, &data_directory).await;
    // Entries nothing names any more, left by a stop between a value and its row (DB-03). Not
    // in a start that switched or put back a restore: the installation kept aside for it still
    // names entries the live database does not, until the restore is finished.
    if matches!(restore, Cutover::Nothing) {
        match database.sweep_vault().await {
            Ok(0) => {}
            Ok(removed) => tracing::info!(removed, "orphaned vault entries removed"),
            Err(error) => {
                tracing::warn!(%error, "the vault was not swept; the next start tries again")
            }
        }
    }
    Ok(Store {
        data_directory,
        database,
        secrets,
        restore,
        instance,
    })
}

/// Moves the archive passwords still in plain columns into the vault, before anything else
/// reads or writes them, and then empties the old columns of the database copies beside it
/// (RD-190-04).
///
/// A failure does not stop the start: the values stay where they were, nothing is lost, and the
/// next start tries again. The copies are only scrubbed once the live database holds none.
async fn take_over_archive_passwords(database: &Database, data_directory: &Path) {
    match database.take_over_archive_passwords().await {
        Ok(moved) => {
            if moved > 0 {
                tracing::info!(moved, "archive passwords moved into the vault");
            }
        }
        Err(error) => {
            tracing::error!(
                %error,
                code = "db.archive_password_takeover_failed",
                "archive passwords stay in the database until the next start"
            );
            return;
        }
    }
    for folder in [
        rd_db::pre_migration::DIRECTORY,
        rd_backup::pre_update::DIRECTORY,
    ] {
        let scrubbed =
            rd_db::pre_migration::scrub_archive_passwords(&data_directory.join(folder)).await;
        if scrubbed > 0 {
            tracing::info!(
                scrubbed,
                folder,
                "archive passwords removed from database copies"
            );
        }
    }
}

/// The first start with a restored state completed: the previous installation, kept until
/// now, goes (RD-160-03).
pub(crate) fn finish_restore(paths: &CommonPaths, restore: &Cutover) {
    if !matches!(restore, Cutover::Switched(_)) {
        return;
    }
    match cutover::finish(&cutover::Layout::new(&paths.database)) {
        Ok(()) => tracing::info!("the restored installation started; the previous one is removed"),
        Err(error) => tracing::warn!(%error, "the finished restore could not be recorded"),
    }
}

/// The runners the queue starts with and the services that stand behind them.
pub(crate) struct NativeRunners {
    pub(crate) runners: Vec<std::sync::Arc<dyn rd_scheduler::ExternalRunner>>,
    pub(crate) media_settings: rd_media::SharedMediaSettings,
    pub(crate) media_probe: std::sync::Arc<dyn rd_media::MediaProbe>,
    pub(crate) gallery_settings: rd_gallery::SharedGallerySettings,
    pub(crate) stream_settings: rd_stream::SharedStreamSettings,
    pub(crate) torrent_service: rd_torrent::TorrentService,
    pub(crate) torrent_settings: rd_torrent::SharedTorrentSettings,
    pub(crate) remote: rd_api::RemoteServices,
    pub(crate) plugin_transfer_schemes: Vec<String>,
    pub(crate) plugin_registry: rd_plugin_host::PluginTypeRegistry,
    /// What the Usenet runner counts per server; `serve` flushes it (RD-1100-05).
    pub(crate) usenet_traffic: rd_usenet::UsenetTraffic,
}

/// Builds every runner the scheduler is started with, and loads the plugin registry the
/// transfer backends and the extensions come from.
pub(crate) async fn native_runners(
    database: &Database,
    secrets: &rd_secrets::SecretStore,
    config: &SchedulerConfig,
    bandwidth: &rd_scheduler::BandwidthService,
    data_directory: &Path,
    plugins: &rd_plugin_host::PluginInstaller,
    allow_local_targets: bool,
) -> Result<NativeRunners> {
    let usenet_traffic = rd_usenet::UsenetTraffic::default();
    let usenet_runner: std::sync::Arc<dyn rd_scheduler::ExternalRunner> = std::sync::Arc::new(
        rd_usenet::UsenetRunner::new(
            database.clone(),
            secrets.clone(),
            rd_usenet::UsenetRunnerConfig::default(),
        )
        // Without this the operator's custom CA reaches HTTP and FTPS but not their news
        // server, which is the inconsistency `rd_http::tls_client_config` exists to stop.
        .with_network_defaults(config.network_defaults.clone())
        .with_traffic(usenet_traffic.clone()),
    );
    // yt-dlp, gallery-dl and streamlink open their own sockets: each run is handed its
    // download's proxy and the custom CA, resolved like an HTTP transfer's (RD-1240-08).
    let tool_network = rd_scheduler::ToolNetworkSource::new(
        database.clone(),
        secrets.clone(),
        config.network_defaults.clone(),
    );
    let media_settings = rd_media::shared_settings(database).await?;
    let (media_runner, media_probe) = rd_media::build_with_tool_network(
        database.clone(),
        secrets.clone(),
        media_settings.clone(),
        tool_network.clone(),
    );
    let gallery_settings = rd_gallery::shared_settings(database).await?;
    let gallery_runner = rd_gallery::build_with_tool_network(
        database.clone(),
        gallery_settings.clone(),
        tool_network.clone(),
    );
    let stream_settings = rd_stream::shared_settings(database).await?;
    let stream_runner = rd_stream::build_with_network_defaults(
        database.clone(),
        stream_settings.clone(),
        config.network_defaults.clone(),
        tool_network,
    );
    let torrent_settings = rd_torrent::shared_settings(database).await?;
    let torrent_service = rd_torrent::TorrentService::start(
        database.clone(),
        torrent_settings.clone(),
        data_directory.to_path_buf(),
        config.downloads_directory.clone(),
    )
    // Needed to resolve the password of a configured SOCKS5 peer proxy.
    .with_secrets(secrets.clone())
    .with_bandwidth(bandwidth.clone());
    let torrent_runner = rd_torrent::build(torrent_service.clone());
    let remote_settings = rd_ftp::shared_settings(database).await?;
    // Built before the scheduler because their runners are registered with it at start.
    let remote = rd_api::RemoteServices::new(
        database.clone(),
        secrets.clone(),
        remote_settings,
        config.network_defaults.clone(),
    );
    // Transfer backends are compiled before the scheduler starts, for the same reason the
    // native runners are built here: the registry is fixed once the queue is running.
    // Its own flag, not development mode: relaxing the target check lets *every* installed
    // backend reach the loopback interface and the private ranges around this host, which has
    // nothing to do with accepting an unsigned package and must be asked for separately.
    // Verified once, here, and handed to every adapter below. Each of them used to call
    // `load_verified` for itself, and that is an Ed25519 check plus a full wasmparser
    // validation plus a fresh `SandboxEngine` — epoch-ticker thread and all — plus a compile,
    // for *every* installed package rather than the type being asked for. Five adapters times
    // thirty plugins is three hundred of those on every start.
    let plugin_registry = rd_plugin_host::PluginTypeRegistry::load(plugins).await?;
    let transfer_backends =
        rd_plugin_transfer::TransferBackends::from_registry(&plugin_registry, allow_local_targets);
    let plugin_transfer_schemes = transfer_backends.schemes();
    if !plugin_transfer_schemes.is_empty() {
        tracing::info!(
            schemes = %plugin_transfer_schemes.join(", "),
            "installed transfer backends"
        );
    }
    let mut runners = vec![
        usenet_runner,
        media_runner,
        gallery_runner,
        stream_runner,
        torrent_runner,
        rd_ftp::build(remote.ftp.clone()),
        rd_sftp::build(remote.sftp.clone()),
        rd_object_storage::build(remote.object_storage.clone()),
    ];
    // Registered only when something is installed, so an unused kind cannot occupy a queue
    // slot or answer for a scheme nothing serves.
    if !transfer_backends.is_empty() {
        runners.push(rd_plugin_transfer::build(
            transfer_backends,
            database.clone(),
            config.network_defaults.read().await.custom_ca_pem.clone(),
        ));
    }
    Ok(NativeRunners {
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
    })
}

/// The plugin extensions the state carries, and the post-processing that runs their steps.
pub(crate) struct PluginServices {
    pub(crate) plugin_steps: std::sync::Arc<rd_plugin_ext::PluginSteps>,
    pub(crate) storage_destinations: std::sync::Arc<rd_plugin_ext::StorageDestinations>,
    pub(crate) extraction: rd_extract::ExtractionService,
    pub(crate) intake_parsers: std::sync::Arc<rd_plugin_ext::IntakeParsers>,
    pub(crate) crawlers: rd_plugin_ext::FolderCrawlers,
    pub(crate) auth_providers: rd_plugin_ext::AuthProviders,
    pub(crate) oauth_providers: rd_plugin_ext::OAuthProviders,
}

/// Compiles the plugin extensions out of `registry` and starts post-processing with them.
pub(crate) fn plugin_services(
    registry: &rd_plugin_host::PluginTypeRegistry,
    scheduler: &SchedulerHandle,
    database: &Database,
    data_directory: &Path,
    postprocess_hold: rd_core::PostprocessHold,
    quiet_hold: rd_core::PostprocessHold,
    object_storage: rd_object_storage::ObjectStorageService,
) -> PluginServices {
    let plugin_steps = std::sync::Arc::new(rd_plugin_ext::PluginSteps::from_registry(
        registry,
        Some(scheduler.plugin_host()),
    ));
    let step_runner: Option<std::sync::Arc<dyn rd_extract::PluginStepRunner>> =
        (!plugin_steps.is_empty()).then(|| {
            std::sync::Arc::clone(&plugin_steps) as std::sync::Arc<dyn rd_extract::PluginStepRunner>
        });
    let storage_destinations = std::sync::Arc::new(
        rd_plugin_ext::StorageDestinations::from_registry(registry, Some(scheduler.plugin_host())),
    );
    let uploader: Option<std::sync::Arc<dyn rd_extract::StorageUploader>> =
        (!storage_destinations.is_empty()).then(|| {
            std::sync::Arc::clone(&storage_destinations)
                as std::sync::Arc<dyn rd_extract::StorageUploader>
        });
    let extraction = rd_extract::ExtractionService::start_with_plugins(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: data_directory.join("passwords.txt"),
            rar_timeout: std::time::Duration::from_secs(30 * 60),
            default_scripts_directory: data_directory.join("scripts"),
            hold: postprocess_hold,
            quiet_hold,
            upload_limit: Some(scheduler.bandwidth().upload_limiter()),
        },
        step_runner,
        uploader,
        Some(std::sync::Arc::new(object_storage) as std::sync::Arc<dyn rd_extract::ObjectUploader>),
    );
    // Built before the state so a parser that fails to compile costs its own feature and
    // nothing else: intake still works, the failure is logged, the service starts.
    let intake_parsers = std::sync::Arc::new(rd_plugin_ext::IntakeParsers::from_registry(
        registry,
        Some(scheduler.plugin_host()),
    ));
    if !intake_parsers.is_empty() {
        tracing::info!("intake parser plugins loaded");
    }
    // Same rule for the folder crawlers (RD-104-03): one that fails to compile costs its own
    // feature, and the LinkGrabber goes on doing exactly what it did before there were any.
    let crawlers =
        rd_plugin_ext::FolderCrawlers::from_registry(registry, Some(scheduler.plugin_host()));
    // The account page's provider catalogue asks which plugins can sign a provider in. Built
    // here from the same registry, it used to load and compile every installed package again
    // on the page's first open (RD-130-06).
    let auth_providers =
        rd_plugin_ext::AuthProviders::from_registry(registry, Some(scheduler.plugin_host()));
    let oauth_providers =
        rd_plugin_ext::OAuthProviders::from_registry(registry, Some(scheduler.plugin_host()));
    PluginServices {
        plugin_steps,
        storage_destinations,
        extraction,
        intake_parsers,
        crawlers,
        auth_providers,
        oauth_providers,
    }
}

/// Puts the site rules behind the folder crawlers.
pub(crate) async fn with_site_rules(
    crawlers: rd_plugin_ext::FolderCrawlers,
    database: &Database,
    secrets: &rd_secrets::SecretStore,
    scheduler: &SchedulerHandle,
) -> std::sync::Arc<rd_plugin_ext::FolderCrawlers> {
    let mut rule_runner = rd_plugin_ext::HostRuleRunner::new(rd_plugin_host::RuleNetwork::new(
        database.clone(),
        secrets.clone(),
        scheduler.network_defaults(),
    ))
    .with_captcha(std::sync::Arc::new(scheduler.captcha()));
    // The value a two-stage rule sends where a page's script sends a fingerprint (RD-1170-03).
    if let Some(device_id) = rd_api::site_rules_service::device_id(database).await {
        rule_runner = rule_runner.with_device_id(device_id);
    }
    // The examples for free sites, switched off, at the first start only (RD-1230-03).
    rd_api::site_rules_service::install_examples_once(database).await;
    let site_rules = std::sync::Arc::new(rd_plugin_ext::SiteRules::new(
        site_rules_cli::load_catalogue(database).await,
        std::sync::Arc::new(rule_runner),
    ));
    // A rule the last self-test found dead is not asked again (RD-110-09). It stays in the
    // catalogue and a later run revives it; what it does not do is cost a request per paste.
    site_rules.set_dead(doctor_site_rules::dead_rules(database).await);
    std::sync::Arc::new(crawlers.with_rules(site_rules))
}

/// Prepares what the state needs before it serves: managed tools, plugin repositories and the
/// hot folders.
pub(crate) async fn prepare_state(state: &AppState) -> Result<()> {
    // Managed external tools (RD-102-02) before anything can resolve a tool: the store reads
    // its pointers and clears an interrupted install's staging directories, and only then is
    // it registered as the managed stage of `rd_core::locate_tool`.
    // Falls back to defaults rather than refusing, as it did before: a bad tool setting must
    // not keep the service from starting, and the accessor now reports what it rejected.
    let managed_tools: rd_core::ManagedToolSettings =
        state.database.service_settings_or_default().await?;
    state.prepare_managed_tools(managed_tools).await;
    // Plugin repositories (RD-140-01): the cached indexes that still verify, and the refresh
    // loop, whose first run waits so it never competes with the start.
    // Automatic updates follow the policy the plugin manager stores per plugin (RD-140-02).
    state
        .plugin_repositories
        .set_update_policy(std::sync::Arc::new(rd_api::VersionChoicePolicy::new(
            state.database.clone(),
        )));
    rd_api::prepare_plugin_repositories(state).await;
    // The application update check (RD-180-01): spawned, its first run minutes after the start,
    // so it never holds the start up.
    state.updates.start();
    // The automatic install (RD-1240-27): a look a minute, which does nothing while the
    // setting is off or this installation does not install itself.
    rd_api::update_auto_install::start(state);
    // The automatic restart (RD-1240-32): remembers what this start found pending already, then
    // a look a minute, which does nothing while `restart_when_needed` is off or nothing waits.
    rd_api::restart_auto::start(state);
    // Old update backups and compiled plugin code nothing uses (RD-1240-34): ten minutes on, once
    // the start's compiles are recorded and an update the updater proves is proven.
    rd_api::system_cleanup::start(state);
    state.hotfolders.start_existing().await?;
    Ok(())
}
