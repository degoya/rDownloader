//! The shared [`AppState`] (RD-1420-06): the ground every area reads, one part per area, built
//! by [`AppStateBuilder`] and finished by the `with_*` steps after it.

mod parts;
mod services;

pub use parts::{AccessState, AdminState, IntakeState, QueueState, RuntimeState};

use super::*;

/// Shared state cloned into request handlers.
///
/// The database, the scheduler and the vault are what every area reads; everything else lives
/// in the part of the area that owns it, so a handler reaches it as `state.<part>.<field>`
/// (RD-1420-06).
#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub scheduler: SchedulerHandle,
    pub secrets: rd_secrets::SecretStore,
    /// Sign-in, sessions, the proxy contract and the ceremonies in flight.
    pub access: AccessState,
    /// The online check, hot folders, subscriptions and the installed parsers and crawlers.
    pub intake: IntakeState,
    /// Unpacking, the torrent engine, the remote transports, power and reconnects.
    pub queue: QueueState,
    /// Plugins, managed tools, the update, notifications and automations.
    pub admin: AdminState,
    /// The build, the local control token, the stop and the restart.
    pub runtime: RuntimeState,
}

/// The services the binary initialised, by name, from which [`Self::build`] starts the
/// background services the [`AppState`] carries.
pub struct AppStateBuilder {
    pub database: Database,
    pub scheduler: SchedulerHandle,
    pub secrets: rd_secrets::SecretStore,
    pub plugins: rd_plugin_host::PluginInstaller,
    pub extraction: rd_extract::ExtractionService,
    pub media_settings: rd_media::SharedMediaSettings,
    pub media_probe: std::sync::Arc<dyn rd_media::MediaProbe>,
    pub gallery_settings: rd_gallery::SharedGallerySettings,
    pub stream_settings: rd_stream::SharedStreamSettings,
    pub torrent: rd_torrent::TorrentService,
    pub torrent_settings: rd_torrent::SharedTorrentSettings,
    pub power: rd_power::PowerService,
    pub quiet_hold: rd_core::PostprocessHold,
    pub remote: RemoteServices,
}

impl AppStateBuilder {
    /// Starts the background services and assembles the state from them.
    #[must_use]
    pub fn build(self) -> AppState {
        let Self {
            database,
            scheduler,
            secrets,
            plugins,
            extraction,
            media_settings,
            media_probe,
            gallery_settings,
            stream_settings,
            torrent,
            torrent_settings,
            power,
            quiet_hold,
            remote,
        } = self;
        // Built before the hotfolders: a `.dlc` dropped into a folder is checked through the
        // same service a pasted batch goes through.
        let link_check = link_check_service::LinkCheckService::start(
            database.clone(),
            scheduler.clone(),
            media_probe.clone(),
            remote.ftp.clone(),
            remote.sftp.clone(),
            remote.object_storage.clone(),
            torrent.clone(),
            plugins.clone(),
            scheduler.plugin_host(),
        );
        let hotfolders = HotFolderService::new(
            database.clone(),
            scheduler.clone(),
            torrent.clone(),
            secrets.clone(),
            link_check.clone(),
            media_settings.clone(),
            gallery_settings.clone(),
        );
        let stream_monitor = stream_monitor::StreamMonitorService::start(
            database.clone(),
            secrets.clone(),
            scheduler.clone(),
            stream_settings.clone(),
        );
        let site_rule_claims = subscription_service::SharedSiteRules::default();
        let subscription_adapters = services::subscription_adapters(
            &database,
            &media_probe,
            &scheduler,
            &secrets,
            &extraction,
            &site_rule_claims,
        );
        let subscriptions = subscription_service::SubscriptionService::start(
            database.clone(),
            link_check.clone(),
            media_settings.clone(),
            gallery_settings.clone(),
            subscription_adapters,
        );
        let quiet_hold = power_service::QuietHold::new(quiet_hold);
        let auth_flows = auth_flow_service::AuthFlowService::start(
            database.clone(),
            plugins.clone(),
            scheduler.plugin_host(),
        );
        let remote_jobs = remote_job_service::RemoteJobService::start(
            database.clone(),
            plugins.clone(),
            scheduler.plugin_host(),
            link_check.clone(),
        );
        link_check.attach_cache_checkers(remote_jobs.clone());
        let notifications = notify_service::NotificationService::start(
            database.clone(),
            secrets.clone(),
            power.clone(),
            scheduler.bandwidth(),
            plugins.clone(),
            scheduler.plugin_host(),
        );
        let automations = automation_service::AutomationService::start(
            database.clone(),
            secrets.clone(),
            scheduler.clone(),
            extraction.clone(),
            automation_links::LinkIntake {
                link_check: link_check.clone(),
                media_settings: media_settings.clone(),
                gallery_settings: gallery_settings.clone(),
            },
        );
        let tools = services::managed_tools(&database);
        let plugin_repositories = services::plugin_repositories(&database, &plugins);
        let capture_agents = capture_agents::CaptureAgents::default();
        let updates = update_service::UpdateService::new(database.clone(), capture_agents.clone());
        let power_supervisor = power_service::PowerSupervisor::start(
            database.clone(),
            scheduler.clone(),
            extraction.clone(),
            power.clone(),
            quiet_hold.clone(),
        );
        AppState {
            database,
            scheduler,
            secrets,
            access: AccessState::new(auth_flows),
            intake: IntakeState {
                hotfolders,
                link_check,
                media_settings,
                gallery_settings,
                stream_settings,
                stream_monitor,
                subscriptions,
                plugin_transfer_schemes: std::sync::Arc::new(Vec::new()),
                intake_parsers: std::sync::Arc::new(rd_plugin_ext::IntakeParsers::none()),
                crawlers: std::sync::Arc::new(rd_plugin_ext::FolderCrawlers::none()),
                site_rule_claims,
                local_capture_fetches: false,
            },
            queue: QueueState {
                extraction,
                torrent,
                torrent_settings,
                ftp: remote.ftp,
                sftp: remote.sftp,
                object_storage: remote.object_storage,
                remote_settings: remote.settings,
                power,
                quiet_hold,
                power_supervisor,
                remote_jobs,
                reconnect: reconnect_service::ReconnectService::default(),
            },
            admin: AdminState {
                plugins,
                tools,
                plugin_repositories,
                updates,
                notifications,
                automations,
                plugin_steps: std::sync::Arc::new(rd_plugin_ext::PluginSteps::none()),
                storage_destinations: std::sync::Arc::new(
                    rd_plugin_ext::StorageDestinations::none(),
                ),
                capture_agents,
            },
            runtime: RuntimeState::new(),
        }
    }
}

impl AppState {
    /// Reads the managed tool store from disk and makes it the managed stage of the tool
    /// lookup (RD-102-02).
    ///
    /// Separate from the constructor because it touches the filesystem: the pointers, the
    /// staging leftovers of an interrupted install and the cached manifest all have to be
    /// read before anything resolves a tool, and the resolver is registered only afterwards
    /// so no job can see a half-loaded store. A test that never calls this simply has no
    /// managed stage, which is exactly the behaviour of an installation that manages nothing.
    pub async fn prepare_managed_tools(&self, settings: rd_core::ManagedToolSettings) {
        self.admin.tools.apply_settings(settings);
        self.admin.tools.load().await;
        rd_core::set_managed_tool_resolver(std::sync::Arc::new(self.admin.tools.clone()));
    }

    /// Records the installed post-processing step plugins.
    #[must_use]
    pub fn with_plugin_steps(mut self, steps: std::sync::Arc<rd_plugin_ext::PluginSteps>) -> Self {
        self.admin.plugin_steps = steps;
        self
    }

    /// Records the installed upload destination plugins.
    #[must_use]
    pub fn with_storage_destinations(
        mut self,
        destinations: std::sync::Arc<rd_plugin_ext::StorageDestinations>,
    ) -> Self {
        self.admin.storage_destinations = destinations;
        self
    }

    /// Records the authentication and OAuth providers the start built (RD-130-06), so the
    /// provider catalogue does not load and compile every installed plugin a second time.
    #[must_use]
    pub fn with_auth_providers(
        self,
        providers: rd_plugin_ext::AuthProviders,
        oauth: rd_plugin_ext::OAuthProviders,
    ) -> Self {
        self.access.auth_flows.preload(providers, oauth);
        self
    }

    /// Records the installed intake parsers.
    ///
    /// Separate from the constructor for the same reason the transfer schemes are: they are
    /// discovered from the plugin directory rather than configured, and a service that
    /// cannot build one still has to start.
    #[must_use]
    pub fn with_intake_parsers(
        mut self,
        parsers: std::sync::Arc<rd_plugin_ext::IntakeParsers>,
    ) -> Self {
        self.intake.intake_parsers = parsers;
        self
    }

    /// Records the installed folder crawlers (RD-104-03).
    ///
    /// Discovered rather than configured, exactly like the intake parsers: a service with
    /// none behaves as it did before there were any.
    #[must_use]
    pub fn with_crawlers(
        mut self,
        crawlers: std::sync::Arc<rd_plugin_ext::FolderCrawlers>,
    ) -> Self {
        // The same rules the crawler selection uses, so a watched listing and a pasted
        // address never disagree about which links are release pages (RD-110-21).
        self.intake
            .site_rule_claims
            .set(crawlers.rules().map(std::sync::Arc::clone));
        self.intake.crawlers = crawlers;
        self
    }

    /// Records the commit and build time the binary was compiled with (RD-130-12).
    ///
    /// Separate from the constructor because only the binary knows them: its build script
    /// compiles them in, and a test router simply has none.
    #[must_use]
    pub fn with_build_info(mut self, build: BuildInfo) -> Self {
        self.runtime.build = std::sync::Arc::new(build);
        self
    }

    /// Records the local control token this start issued (RD-180-02).
    #[must_use]
    pub fn with_local_control(mut self, control: local_control::LocalControl) -> Self {
        self.runtime.local_control = control;
        self
    }

    /// Records the token that stops the service, the one the binary's signal handler cancels.
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: tokio_util::sync::CancellationToken) -> Self {
        self.runtime.shutdown = shutdown;
        self
    }

    /// Records the URL schemes the installed transfer backends claim.
    ///
    /// Separate from the constructor because it is discovered rather than configured: the
    /// backends are compiled at startup, and a service with none behaves exactly as before.
    #[must_use]
    pub fn with_plugin_transfer_schemes(mut self, schemes: Vec<String>) -> Self {
        self.intake.plugin_transfer_schemes = std::sync::Arc::new(schemes);
        self
    }

    /// Lets `capture/file` fetch an address on this machine, which it otherwise refuses
    /// (security review 2026-09-28, finding 9).
    ///
    /// For tests only, whose "indexers" listen on loopback: the service never calls it, and no
    /// setting reaches it.
    #[doc(hidden)]
    #[must_use]
    pub fn with_local_capture_fetches(mut self) -> Self {
        self.intake.local_capture_fetches = true;
        self
    }

    /// Checks open event streams' credentials this often instead of every thirty seconds.
    ///
    /// For tests, which cannot wait half a minute to see a stream end: the service never calls
    /// it, and no setting reaches it.
    #[doc(hidden)]
    #[must_use]
    pub fn with_stream_recheck(mut self, every: std::time::Duration) -> Self {
        self.runtime.stream_recheck = every;
        self
    }
}
