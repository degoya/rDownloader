//! The ground every part of the HTTP surface stands on (RD-160-06): the shared [`AppState`] and
//! the background services it carries, the error type and its stable codes, the DTOs, the
//! authentication and scope policy, the audit log, and the helpers more than one area needs.
//!
//! The areas themselves — `rd-api-access`, `rd-api-intake`, `rd-api-queue`, `rd-api-admin` —
//! build on this crate and not on each other, so rustc compiles them side by side, and
//! `rd-api-compat`, `rd-api-mcp` and `rd-api` assemble them.

pub mod audit;
pub mod auth;
pub mod auth_flow_service;
pub mod automation_actions;
pub mod automation_context;
pub mod automation_input;
pub mod automation_service;
pub mod browser_session;
pub mod build_info;
pub mod capture_sanitize;
pub mod client;
pub mod collector_exclusions;
pub mod collector_intake;
pub mod config_fields;
pub mod container_upload;
pub mod destination;
pub mod dlc_import;
pub mod dto;
pub mod error;
pub mod error_codes;
pub mod host_check;
pub mod hosters;
pub mod hotfolder_service;
pub mod link_check_cache;
pub mod link_check_probe;
pub mod link_check_service;
pub mod notify_service;
pub mod postprocess_handlers;
pub mod power_service;
pub mod qbittorrent_sessions;
pub mod reconnect_decision;
pub mod reconnect_ip;
pub mod reconnect_service;
pub mod remote_job_service;
pub mod scope_policy;
pub mod settings_store;
pub mod storage_capacity;
pub mod stream_monitor;
pub mod subscription_hosts;
pub mod subscription_service;
pub mod torrent_intake;
pub mod trace_context;

use rd_db::Database;
use rd_scheduler::SchedulerHandle;

pub use auth::AuthService;
pub use build_info::BuildInfo;
pub use error::ApiError;
pub use hotfolder_service::HotFolderService;
pub use link_check_service::LinkCheckService;
pub use remote_job_service::{
    ChoiceOutcome as RemoteJobChoiceOutcome, DiscardOutcome as RemoteJobDiscardOutcome,
    RemoteJobRefused, RemoteJobService, SubmitOutcome as RemoteJobSubmitOutcome,
};
pub use scope_policy::{policy_rows, required_scope};
pub use settings_store::service_switches;

/// Shared state cloned into request handlers.
#[derive(Clone)]
pub struct AppState {
    pub database: Database,
    pub scheduler: SchedulerHandle,
    pub auth: AuthService,
    pub hotfolders: HotFolderService,
    pub secrets: rd_secrets::SecretStore,
    pub plugins: rd_plugin_host::PluginInstaller,
    pub extraction: rd_extract::ExtractionService,
    pub link_check: link_check_service::LinkCheckService,
    /// Media (yt-dlp) settings shared with the runner and the probe.
    pub media_settings: rd_media::SharedMediaSettings,
    /// Gallery (gallery-dl) settings shared with the runner.
    pub gallery_settings: rd_gallery::SharedGallerySettings,
    /// Recording (streamlink) settings shared with the runner and the channel monitor.
    pub stream_settings: rd_stream::SharedStreamSettings,
    /// Managed external tools (RD-102-02): the signed manifest, the installed versions and
    /// which one the tool lookup answers with.
    pub tools: rd_tools::ManagedToolService,
    /// Signed plugin repositories (RD-140-01): their verified indexes, offers and updates.
    pub plugin_repositories: rd_plugin_host::repository::PluginRepositoryService,
    /// Background monitor that starts recordings when watched channels go live.
    pub stream_monitor: stream_monitor::StreamMonitorService,
    pub subscriptions: subscription_service::SubscriptionService,
    /// Embedded BitTorrent engine (session, seeding supervision).
    pub torrent: rd_torrent::TorrentService,
    /// Torrent settings shared with the engine (port/limits apply on next start).
    pub torrent_settings: rd_torrent::SharedTorrentSettings,
    /// Quiet hours, completion actions and the platform power/network context.
    pub power: rd_power::PowerService,
    /// Raised while quiet hours defer the resource-intensive post-processing steps.
    pub quiet_hold: power_service::QuietHold,
    /// Background loop that watches the queue for a completed work cycle.
    pub power_supervisor: power_service::PowerSupervisor,
    /// Notification hub: event mapping and the delivery worker.
    pub notifications: notify_service::NotificationService,
    /// Automation engine: event intake, run queue and action execution.
    pub automations: automation_service::AutomationService,
    /// FTP/FTPS transport, used by the online check and the credential test action.
    pub ftp: rd_ftp::FtpService,
    /// SFTP transport, which also owns the SSH host-key trust decisions.
    pub sftp: rd_sftp::SftpService,
    /// Object storage (RD-150-04): the probe, the profile test and the upload sweep.
    pub object_storage: rd_object_storage::ObjectStorageService,
    /// Remote transfer settings shared with both runners.
    pub remote_settings: rd_ftp::SharedRemoteSettings,
    /// URL schemes the installed transfer backends claim, so intake can route a link to the
    /// plugin runner. Read once at startup: a newly installed backend needs a restart before
    /// its resolver or runner exists anyway.
    pub plugin_transfer_schemes: std::sync::Arc<Vec<String>>,
    /// Installed intake parsers (RD-090-12). Read once at startup, like the transfer
    /// schemes: a newly installed parser needs a restart before it can be built anyway.
    pub intake_parsers: std::sync::Arc<rd_plugin_ext::IntakeParsers>,
    /// Installed folder crawlers (RD-104-03), read once at startup like the parsers. They
    /// turn a pasted folder address into the files behind it before anything else looks at
    /// the link.
    pub crawlers: std::sync::Arc<rd_plugin_ext::FolderCrawlers>,
    /// The site rules a watched release page asks about its listing (RD-110-21). Filled by
    /// [`AppState::with_crawlers`], because the poll loop exists before the rules do.
    pub site_rule_claims: subscription_service::SharedSiteRules,
    /// Provider sign-in flows run by authentication plugins (RD-090-13).
    pub auth_flows: auth_flow_service::AuthFlowService,
    /// Jobs that run at a provider (RD-108-03): the sweep that drives them and the entry
    /// point that starts one.
    pub remote_jobs: remote_job_service::RemoteJobService,
    pub reconnect: reconnect_service::ReconnectService,
    /// Requests for a browser's session at a provider, opened in the web interface and
    /// answered by the extension (RD-120-45). In memory only.
    pub browser_sessions: browser_session::BrowserSessions,
    /// Installed post-processing step plugins (RD-090-16), for the settings and category
    /// editors. The pipeline reaches them through the runner it was started with.
    pub plugin_steps: std::sync::Arc<rd_plugin_ext::PluginSteps>,
    /// Installed upload destination plugins (RD-090-17), for the settings editor. The
    /// pipeline reaches them through the uploader it was started with.
    pub storage_destinations: std::sync::Arc<rd_plugin_ext::StorageDestinations>,
    /// The contract with whatever sits in front of the service: which hops may speak for a
    /// client, what the outside world calls this installation, and whether cookies are Secure.
    ///
    /// Empty by default, and that default is the safe one: with nothing configured the
    /// socket's peer address is the client and no header can change it. Behind a lock because
    /// the setting is editable while the service runs.
    pub proxy: std::sync::Arc<tokio::sync::RwLock<rd_authn::ProxyConfig>>,
    /// Passkey enrolments waiting for the browser's half of the ceremony.
    ///
    /// In memory rather than in the database: `webauthn-rs` refuses to serialise this state
    /// without an explicitly dangerous feature flag, because a challenge that survives a
    /// restart is a challenge that can be replayed into the process that comes back.
    pub passkey_registrations:
        std::sync::Arc<rd_authn::CeremonyStore<webauthn_rs::prelude::PasskeyRegistration>>,
    /// Passkey sign-ins waiting for the same. Reachable without a session, hence bounded.
    pub passkey_authentications:
        std::sync::Arc<rd_authn::CeremonyStore<webauthn_rs::prelude::PasskeyAuthentication>>,
    /// Live `SID` handles of the qBittorrent adapter.
    ///
    /// In memory, like the passkey ceremonies above and for a related reason: a handle that
    /// cannot outlive the process it was minted in cannot be replayed into the next one. The
    /// cost is that an automation client logs in again after a restart, which is what real
    /// qBittorrent makes it do anyway.
    pub qbittorrent_sessions: std::sync::Arc<qbittorrent_sessions::SessionStore>,
    /// Commit and build time for the About page (RD-130-12); empty unless the binary sets them.
    pub build: std::sync::Arc<BuildInfo>,
    /// Lets `capture/file` fetch from this machine; see [`Self::with_local_capture_fetches`].
    pub local_capture_fetches: bool,
}

/// The milestone 0.6 transfer services, grouped so they travel as one argument.
#[derive(Clone)]
pub struct RemoteServices {
    pub ftp: rd_ftp::FtpService,
    pub sftp: rd_sftp::SftpService,
    pub object_storage: rd_object_storage::ObjectStorageService,
    pub settings: rd_ftp::SharedRemoteSettings,
}

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
        // The media adapter reuses the existing probe rather than talking to yt-dlp a second
        // way; RD-080-10, RD-080-11, RD-110-21 and RD-130-19 add their adapters to this same list.
        let site_rule_claims = subscription_service::SharedSiteRules::default();
        let subscription_adapters: Vec<std::sync::Arc<dyn rd_subscription::SourceAdapter>> = vec![
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
        ];
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
        // Rooted at the data directory when the binary registered one. `data/` is the same
        // default the CLI uses, so a test that never sets one reads an empty store rather
        // than an absolute path from somewhere else.
        let tools = rd_tools::ManagedToolService::new(
            database.clone(),
            rd_tools::store_root(
                rd_core::data_directory().unwrap_or_else(|| std::path::Path::new("data")),
            ),
            rd_core::ManagedToolSettings::default(),
        );
        let plugin_repositories = rd_plugin_host::repository::PluginRepositoryService::new(
            database.clone(),
            rd_core::data_directory()
                .unwrap_or_else(|| std::path::Path::new("data"))
                .join("plugin-repositories"),
            plugins.clone(),
        );
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
}
