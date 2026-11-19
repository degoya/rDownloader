//! The parts of the [`AppState`](crate::AppState), one per area of the HTTP surface
//! (RD-1420-06): what a handler of that area reaches through `state.<part>`.

use std::sync::Arc;

use crate::{
    AuthService, BuildInfo, HotFolderService, auth_flow_service, automation_service,
    browser_session, capture_agents, link_check_service, local_control, notify_service,
    oidc_client, power_service, qbittorrent_sessions, reconnect_service, remote_job_service,
    restart_state, stream_monitor, subscription_service, update_service,
};

/// Sign-in and who may speak: sessions, the proxy contract, the ceremonies in flight and the
/// provider sign-in flows.
#[derive(Clone)]
pub struct AccessState {
    pub auth: AuthService,
    /// The contract with whatever sits in front of the service: which hops may speak for a
    /// client, what the outside world calls this installation, and whether cookies are Secure.
    ///
    /// Empty by default, and that default is the safe one: with nothing configured the
    /// socket's peer address is the client and no header can change it. Behind a lock because
    /// the setting is editable while the service runs.
    pub proxy: Arc<tokio::sync::RwLock<rd_authn::ProxyConfig>>,
    /// Passkey enrolments waiting for the browser's half of the ceremony.
    ///
    /// In memory rather than in the database: `webauthn-rs` refuses to serialise this state
    /// without an explicitly dangerous feature flag, because a challenge that survives a
    /// restart is a challenge that can be replayed into the process that comes back.
    pub passkey_registrations:
        Arc<rd_authn::CeremonyStore<webauthn_rs::prelude::PasskeyRegistration>>,
    /// Passkey sign-ins waiting for the same. Reachable without a session, hence bounded.
    pub passkey_authentications:
        Arc<rd_authn::CeremonyStore<webauthn_rs::prelude::PasskeyAuthentication>>,
    /// Signing in through an identity provider (RD-190-15): the flows between start and
    /// callback, in memory and bounded like the passkey ceremonies, and the provider's cached
    /// discovery document and keys.
    pub oidc: oidc_client::OidcClient,
    /// Live `SID` handles of the qBittorrent adapter.
    ///
    /// In memory, like the passkey ceremonies above and for a related reason: a handle that
    /// cannot outlive the process it was minted in cannot be replayed into the next one. The
    /// cost is that an automation client logs in again after a restart, which is what real
    /// qBittorrent makes it do anyway.
    pub qbittorrent_sessions: Arc<qbittorrent_sessions::SessionStore>,
    /// Provider sign-in flows run by authentication plugins (RD-090-13).
    pub auth_flows: auth_flow_service::AuthFlowService,
    /// Requests for a browser's session at a provider, opened in the web interface and
    /// answered by the extension (RD-120-45). In memory only.
    pub browser_sessions: browser_session::BrowserSessions,
}

impl AccessState {
    /// Nothing signed in, nothing in flight, no proxy trusted.
    pub(super) fn new(auth_flows: auth_flow_service::AuthFlowService) -> Self {
        Self {
            auth: AuthService::default(),
            proxy: Arc::new(tokio::sync::RwLock::new(rd_authn::ProxyConfig::default())),
            passkey_registrations: Arc::new(rd_authn::CeremonyStore::new()),
            passkey_authentications: Arc::new(rd_authn::CeremonyStore::new()),
            oidc: oidc_client::OidcClient::default(),
            qbittorrent_sessions: Arc::default(),
            auth_flows,
            browser_sessions: browser_session::BrowserSessions::default(),
        }
    }
}

/// How links come in: the online check, hot folders, subscriptions and channel recordings, the
/// tool settings intake builds jobs with, and the installed parsers and crawlers.
#[derive(Clone)]
pub struct IntakeState {
    pub hotfolders: HotFolderService,
    pub link_check: link_check_service::LinkCheckService,
    /// Media (yt-dlp) settings shared with the runner and the probe.
    pub media_settings: rd_media::SharedMediaSettings,
    /// Gallery (gallery-dl) settings shared with the runner.
    pub gallery_settings: rd_gallery::SharedGallerySettings,
    /// Recording (streamlink) settings shared with the runner and the channel monitor.
    pub stream_settings: rd_stream::SharedStreamSettings,
    /// Background monitor that starts recordings when watched channels go live.
    pub stream_monitor: stream_monitor::StreamMonitorService,
    pub subscriptions: subscription_service::SubscriptionService,
    /// URL schemes the installed transfer backends claim, so intake can route a link to the
    /// plugin runner. Read once at startup: a newly installed backend needs a restart before
    /// its resolver or runner exists anyway.
    pub plugin_transfer_schemes: Arc<Vec<String>>,
    /// Installed intake parsers (RD-090-12). Read once at startup, like the transfer
    /// schemes: a newly installed parser needs a restart before it can be built anyway.
    pub intake_parsers: Arc<rd_plugin_ext::IntakeParsers>,
    /// Installed folder crawlers (RD-104-03), read once at startup like the parsers. They
    /// turn a pasted folder address into the files behind it before anything else looks at
    /// the link.
    pub crawlers: Arc<rd_plugin_ext::FolderCrawlers>,
    /// The site rules a watched release page asks about its listing (RD-110-21). Filled by
    /// [`AppState::with_crawlers`](crate::AppState::with_crawlers), because the poll loop
    /// exists before the rules do.
    pub site_rule_claims: subscription_service::SharedSiteRules,
    /// Lets `capture/file` fetch from this machine; see
    /// [`AppState::with_local_capture_fetches`](crate::AppState::with_local_capture_fetches).
    pub local_capture_fetches: bool,
}

/// What moves and finishes files: unpacking, the torrent engine, the remote transports, quiet
/// hours and completion actions, jobs at a provider and reconnects.
#[derive(Clone)]
pub struct QueueState {
    pub extraction: rd_extract::ExtractionService,
    /// Embedded BitTorrent engine (session, seeding supervision).
    pub torrent: rd_torrent::TorrentService,
    /// Torrent settings shared with the engine (port/limits apply on next start).
    pub torrent_settings: rd_torrent::SharedTorrentSettings,
    /// FTP/FTPS transport, used by the online check and the credential test action.
    pub ftp: rd_ftp::FtpService,
    /// SFTP transport, which also owns the SSH host-key trust decisions.
    pub sftp: rd_sftp::SftpService,
    /// Object storage (RD-150-04): the probe, the profile test and the upload sweep.
    pub object_storage: rd_object_storage::ObjectStorageService,
    /// Remote transfer settings shared with both runners.
    pub remote_settings: rd_ftp::SharedRemoteSettings,
    /// Quiet hours, completion actions and the platform power/network context.
    pub power: rd_power::PowerService,
    /// Raised while quiet hours defer the resource-intensive post-processing steps.
    pub quiet_hold: power_service::QuietHold,
    /// Background loop that watches the queue for a completed work cycle.
    pub power_supervisor: power_service::PowerSupervisor,
    /// Jobs that run at a provider (RD-108-03): the sweep that drives them and the entry
    /// point that starts one.
    pub remote_jobs: remote_job_service::RemoteJobService,
    pub reconnect: reconnect_service::ReconnectService,
}

/// What an administrator configures: plugins and their repositories, managed tools, the
/// application update, notifications and automations.
#[derive(Clone)]
pub struct AdminState {
    pub plugins: rd_plugin_host::PluginInstaller,
    /// Managed external tools (RD-102-02): the signed manifest, the installed versions and
    /// which one the tool lookup answers with.
    pub tools: rd_tools::ManagedToolService,
    /// Signed plugin repositories (RD-140-01): their verified indexes, offers and updates.
    pub plugin_repositories: rd_plugin_host::repository::PluginRepositoryService,
    /// The application update check (RD-180-01): the signed manifests, the replay floors and
    /// what this installation is offered.
    pub updates: update_service::UpdateService,
    /// Notification hub: event mapping and the delivery worker.
    pub notifications: notify_service::NotificationService,
    /// Automation engine: event intake, run queue and action execution.
    pub automations: automation_service::AutomationService,
    /// Installed post-processing step plugins (RD-090-16), for the settings and category
    /// editors. The pipeline reaches them through the runner it was started with.
    pub plugin_steps: Arc<rd_plugin_ext::PluginSteps>,
    /// Installed upload destination plugins (RD-090-17), for the settings editor. The
    /// pipeline reaches them through the uploader it was started with.
    pub storage_destinations: Arc<rd_plugin_ext::StorageDestinations>,
    /// The capture agents holding their event stream open and the version each reported
    /// (RD-190-07), for the update status. In memory only.
    pub capture_agents: capture_agents::CaptureAgents,
}

/// The running process itself: its build, the local control token, the stop and the restart.
#[derive(Clone)]
pub struct RuntimeState {
    /// Commit and build time for the About page (RD-130-12); empty unless the binary sets them.
    pub build: Arc<BuildInfo>,
    /// The token a launcher or the updater on this machine stops the service with and asks
    /// for the backup before an update (RD-180-02, RD-180-03); accepts nothing until the binary
    /// issues one.
    pub local_control: local_control::LocalControl,
    /// Cancelled to stop the service gracefully: the listener stops, then the queue checkpoints
    /// and the binary exits. The binary hands in the token its signal handler cancels too.
    pub shutdown: tokio_util::sync::CancellationToken,
    /// A pending restart and the restart under way (RD-1240-32): what waits for the next start,
    /// how this installation restarts, and the exit code a supervised restart ends with.
    pub restart: restart_state::RestartState,
    /// How often an open event stream checks that its credential still stands
    /// (`stream_standing`); see
    /// [`AppState::with_stream_recheck`](crate::AppState::with_stream_recheck).
    pub stream_recheck: std::time::Duration,
}

impl RuntimeState {
    /// No build information, no control token, nothing stopped or pending.
    pub(super) fn new() -> Self {
        Self {
            build: Arc::default(),
            local_control: local_control::LocalControl::default(),
            shutdown: tokio_util::sync::CancellationToken::new(),
            restart: restart_state::RestartState::default(),
            stream_recheck: crate::stream_standing::STREAM_RECHECK,
        }
    }
}
