//! Building the [`AppState`] and the services it carries, and the builder steps after it.

use super::*;

impl RemoteServices {
    /// Builds the transports on the scheduler's network defaults, so a proxy and a custom
    /// CA reach them exactly as they reach HTTP. Byte pacing is not passed here: the queue
    /// hands each runner a limiter scoped to the transfer it is about to start.
    #[must_use]
    pub fn new(
        database: Database,
        secrets: rd_secrets::SecretStore,
        settings: rd_ftp::SharedRemoteSettings,
        network_defaults: rd_http::SharedNetworkDefaults,
    ) -> Self {
        Self {
            ftp: rd_ftp::FtpService::new(
                database.clone(),
                secrets.clone(),
                settings.clone(),
                network_defaults.clone(),
            ),
            object_storage: rd_object_storage::ObjectStorageService::new(
                database.clone(),
                secrets.clone(),
                settings.clone(),
                network_defaults,
            ),
            sftp: rd_sftp::SftpService::new(database, secrets, settings.clone()),
            settings,
        }
    }
}

impl AppState {
    /// Creates API state from initialized service components.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: Database,
        scheduler: SchedulerHandle,
        secrets: rd_secrets::SecretStore,
        plugins: rd_plugin_host::PluginInstaller,
        extraction: rd_extract::ExtractionService,
        media_settings: rd_media::SharedMediaSettings,
        media_probe: std::sync::Arc<dyn rd_media::MediaProbe>,
        gallery_settings: rd_gallery::SharedGallerySettings,
        stream_settings: rd_stream::SharedStreamSettings,
        torrent: rd_torrent::TorrentService,
        torrent_settings: rd_torrent::SharedTorrentSettings,
        power: rd_power::PowerService,
        quiet_hold: rd_core::PostprocessHold,
        remote: RemoteServices,
    ) -> Self {
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
            scheduler.clone(),
            stream_settings.clone(),
        );
        let site_rule_claims = subscription_service::SharedSiteRules::default();
        let subscription_adapters = subscription_adapters(
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
            plugins.clone(),
            scheduler.plugin_host(),
        );
        let automations = automation_service::AutomationService::start(
            database.clone(),
            secrets.clone(),
            scheduler.clone(),
            extraction.clone(),
        );
        let tools = managed_tools(&database);
        let plugin_repositories = plugin_repositories(&database, &plugins);
        let capture_agents = capture_agents::CaptureAgents::default();
        let updates = update_service::UpdateService::new(database.clone(), capture_agents.clone());
        let power_supervisor = power_service::PowerSupervisor::start(
            database.clone(),
            scheduler.clone(),
            extraction.clone(),
            power.clone(),
            quiet_hold.clone(),
        );
        Self {
            database,
            scheduler,
            auth: AuthService::default(),
            proxy: std::sync::Arc::new(tokio::sync::RwLock::new(rd_authn::ProxyConfig::default())),
            passkey_registrations: std::sync::Arc::new(rd_authn::CeremonyStore::new()),
            passkey_authentications: std::sync::Arc::new(rd_authn::CeremonyStore::new()),
            oidc: oidc_client::OidcClient::default(),
            qbittorrent_sessions: std::sync::Arc::default(),
            hotfolders,
            secrets,
            plugins,
            extraction,
            link_check,
            media_settings,
            gallery_settings,
            stream_settings,
            stream_monitor,
            tools,
            plugin_repositories,
            updates,
            subscriptions,
            torrent,
            torrent_settings,
            power,
            quiet_hold,
            power_supervisor,
            notifications,
            auth_flows,
            remote_jobs,
            reconnect: reconnect_service::ReconnectService::default(),
            browser_sessions: browser_session::BrowserSessions::default(),
            capture_agents,
            automations,
            ftp: remote.ftp,
            sftp: remote.sftp,
            object_storage: remote.object_storage,
            remote_settings: remote.settings,
            plugin_transfer_schemes: std::sync::Arc::new(Vec::new()),
            intake_parsers: std::sync::Arc::new(rd_plugin_ext::IntakeParsers::none()),
            crawlers: std::sync::Arc::new(rd_plugin_ext::FolderCrawlers::none()),
            site_rule_claims,
            plugin_steps: std::sync::Arc::new(rd_plugin_ext::PluginSteps::none()),
            storage_destinations: std::sync::Arc::new(rd_plugin_ext::StorageDestinations::none()),
            build: std::sync::Arc::default(),
            local_capture_fetches: false,
            local_control: local_control::LocalControl::default(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            stream_recheck: stream_standing::STREAM_RECHECK,
        }
    }

    /// Reads the managed tool store from disk and makes it the managed stage of the tool
    /// lookup (RD-102-02).
    ///
    /// Separate from the constructor because it touches the filesystem: the pointers, the
    /// staging leftovers of an interrupted install and the cached manifest all have to be
    /// read before anything resolves a tool, and the resolver is registered only afterwards
    /// so no job can see a half-loaded store. A test that never calls this simply has no
    /// managed stage, which is exactly the behaviour of an installation that manages nothing.
    pub async fn prepare_managed_tools(&self, settings: rd_core::ManagedToolSettings) {
        self.tools.apply_settings(settings);
        self.tools.load().await;
        rd_core::set_managed_tool_resolver(std::sync::Arc::new(self.tools.clone()));
    }

    /// Records the installed post-processing step plugins.
    #[must_use]
    pub fn with_plugin_steps(mut self, steps: std::sync::Arc<rd_plugin_ext::PluginSteps>) -> Self {
        self.plugin_steps = steps;
        self
    }

    /// Records the installed upload destination plugins.
    #[must_use]
    pub fn with_storage_destinations(
        mut self,
        destinations: std::sync::Arc<rd_plugin_ext::StorageDestinations>,
    ) -> Self {
        self.storage_destinations = destinations;
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
        self.auth_flows.preload(providers, oauth);
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
        self.intake_parsers = parsers;
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
        self.site_rule_claims
            .set(crawlers.rules().map(std::sync::Arc::clone));
        self.crawlers = crawlers;
        self
    }

    /// Records the commit and build time the binary was compiled with (RD-130-12).
    ///
    /// Separate from the constructor because only the binary knows them: its build script
    /// compiles them in, and a test router simply has none.
    #[must_use]
    pub fn with_build_info(mut self, build: BuildInfo) -> Self {
        self.build = std::sync::Arc::new(build);
        self
    }

    /// Records the local control token this start issued (RD-180-02).
    #[must_use]
    pub fn with_local_control(mut self, control: local_control::LocalControl) -> Self {
        self.local_control = control;
        self
    }

    /// Records the token that stops the service, the one the binary's signal handler cancels.
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: tokio_util::sync::CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Records the URL schemes the installed transfer backends claim.
    ///
    /// Separate from the constructor because it is discovered rather than configured: the
    /// backends are compiled at startup, and a service with none behaves exactly as before.
    #[must_use]
    pub fn with_plugin_transfer_schemes(mut self, schemes: Vec<String>) -> Self {
        self.plugin_transfer_schemes = std::sync::Arc::new(schemes);
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
        self.local_capture_fetches = true;
        self
    }

    /// Checks open event streams' credentials this often instead of every thirty seconds.
    ///
    /// For tests, which cannot wait half a minute to see a stream end: the service never calls
    /// it, and no setting reaches it.
    #[doc(hidden)]
    #[must_use]
    pub fn with_stream_recheck(mut self, every: std::time::Duration) -> Self {
        self.stream_recheck = every;
        self
    }
}

/// The sources a subscription can watch.
///
/// The media adapter reuses the existing probe rather than talking to yt-dlp a second
/// way; RD-080-10, RD-080-11, RD-110-21, RD-130-19 and RD-190-13 add their adapters to
/// this same list.
fn subscription_adapters(
    database: &Database,
    media_probe: &std::sync::Arc<dyn rd_media::MediaProbe>,
    scheduler: &SchedulerHandle,
    secrets: &rd_secrets::SecretStore,
    extraction: &rd_extract::ExtractionService,
    site_rule_claims: &subscription_service::SharedSiteRules,
) -> Vec<std::sync::Arc<dyn rd_subscription::SourceAdapter>> {
    vec![
        std::sync::Arc::new(rd_subscription::MediaAdapter::new(media_probe.clone())),
        std::sync::Arc::new(rd_subscription::FeedAdapter::new(std::sync::Arc::new(
            subscription_service::HttpFeedFetcher::new(scheduler.clone()),
        ))),
        std::sync::Arc::new(rd_subscription::IndexerAdapter::new(
            std::sync::Arc::new(subscription_service::HttpFeedFetcher::new(
                scheduler.clone(),
            )),
            std::sync::Arc::new(subscription_service::VaultSecretResolver::new(
                secrets.clone(),
            )),
            // Where a poll meets what it already has, so it pages no further (RD-1150-05).
            std::sync::Arc::new(subscription_service::ArchivedItems::new(database.clone())),
        )),
        // A watched release or series page (RD-110-21): the listing is fetched
        // conditionally like a feed, and which of its links are release pages is the
        // rules' answer rather than a guess.
        std::sync::Arc::new(rd_subscription::RuleAdapter::new(
            std::sync::Arc::new(subscription_service::HttpFeedFetcher::new(
                scheduler.clone(),
            )),
            std::sync::Arc::new(site_rule_claims.clone()),
        )),
        // An administrator's script whose output lines are links (RD-130-19), run through
        // the same sandbox as every other user script.
        std::sync::Arc::new(rd_subscription::ScriptAdapter::new(std::sync::Arc::new(
            subscription_service::SandboxScriptRunner::new(extraction.clone()),
        ))),
        // The releases of a GitHub or GitLab repository (RD-190-13), read through the
        // forge's API with the subscription's token from the vault, when it has one.
        std::sync::Arc::new(rd_subscription::GitReleaseAdapter::new(
            std::sync::Arc::new(subscription_service::HttpApiFetcher::new(scheduler.clone())),
            std::sync::Arc::new(subscription_service::VaultSecretResolver::new(
                secrets.clone(),
            )),
        )),
    ]
}

/// The managed tool store.
///
/// Rooted at the data directory when the binary registered one. `data/` is the same
/// default the CLI uses, so a test that never sets one reads an empty store rather
/// than an absolute path from somewhere else.
fn managed_tools(database: &Database) -> rd_tools::ManagedToolService {
    rd_tools::ManagedToolService::new(
        database.clone(),
        rd_tools::store_root(
            rd_core::data_directory().unwrap_or_else(|| std::path::Path::new("data")),
        ),
        rd_core::ManagedToolSettings::default(),
    )
}

/// The plugin repositories, kept beside the data directory.
fn plugin_repositories(
    database: &Database,
    plugins: &rd_plugin_host::PluginInstaller,
) -> rd_plugin_host::repository::PluginRepositoryService {
    rd_plugin_host::repository::PluginRepositoryService::new(
        database.clone(),
        rd_core::data_directory()
            .unwrap_or_else(|| std::path::Path::new("data"))
            .join("plugin-repositories"),
        plugins.clone(),
    )
}
