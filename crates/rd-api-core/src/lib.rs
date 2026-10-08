//! The ground every part of the HTTP surface stands on (RD-160-06): the shared [`AppState`] and
//! the background services it carries, the error type and its stable codes, the DTOs, the
//! authentication and scope policy, the audit log, and the helpers more than one area needs.
//!
//! The areas themselves — `rd-api-access`, `rd-api-intake`, `rd-api-queue`, `rd-api-admin` —
//! build on this crate and not on each other, so rustc compiles them side by side, and
//! `rd-api-compat`, `rd-api-mcp` and `rd-api` assemble them.

#![warn(unreachable_pub)]

mod app_state;
pub mod audit;
pub mod auth;
pub mod auth_flow_guard;
pub mod auth_flow_service;
pub mod automation_actions;
pub mod automation_context;
pub mod automation_input;
pub mod automation_service;
pub mod browser_session;
pub mod build_info;
pub mod capture_agents;
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
pub mod input_checks;
pub mod link_check_cache;
pub mod link_check_probe;
pub mod link_check_service;
pub mod list_bounds;
pub mod local_control;
pub mod notify_notice;
pub mod notify_service;
pub mod object_upload_target;
pub mod oidc_client;
pub mod password_reset;
pub mod postprocess_handlers;
pub mod power_service;
pub mod qbittorrent_sessions;
pub mod reconnect_decision;
pub mod reconnect_ip;
pub mod reconnect_service;
pub mod remote_job_service;
pub mod scope_policy;
pub mod settings_store;
pub mod step_up;
pub mod storage_capacity;
pub mod stream_monitor;
pub mod stream_standing;
pub mod subscription_hosts;
pub mod subscription_service;
pub mod torrent_intake;
pub mod trace_context;
pub mod update_service;

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
pub use settings_store::{
    RUNTIME_FIELDS, RefusedSetting, diagnosed_settings, runtime_settings, service_switches,
    startup_settings,
};

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
    /// The application update check (RD-180-01): the signed manifests, the replay floors and
    /// what this installation is offered.
    pub updates: update_service::UpdateService,
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
    /// The capture agents holding their event stream open and the version each reported
    /// (RD-190-07), for the update status. In memory only.
    pub capture_agents: capture_agents::CaptureAgents,
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
    pub qbittorrent_sessions: std::sync::Arc<qbittorrent_sessions::SessionStore>,
    /// Commit and build time for the About page (RD-130-12); empty unless the binary sets them.
    pub build: std::sync::Arc<BuildInfo>,
    /// Lets `capture/file` fetch from this machine; see [`Self::with_local_capture_fetches`].
    pub local_capture_fetches: bool,
    /// The token a launcher or the updater on this machine stops the service with and asks
    /// for the backup before an update (RD-180-02, RD-180-03); accepts nothing until the binary
    /// issues one.
    pub local_control: local_control::LocalControl,
    /// Cancelled to stop the service gracefully: the listener stops, then the queue checkpoints
    /// and the binary exits. The binary hands in the token its signal handler cancels too.
    pub shutdown: tokio_util::sync::CancellationToken,
    /// How often an open event stream checks that its credential still stands
    /// (`stream_standing`); see [`Self::with_stream_recheck`].
    pub stream_recheck: std::time::Duration,
}

/// The milestone 0.6 transfer services, grouped so they travel as one argument.
#[derive(Clone)]
pub struct RemoteServices {
    pub ftp: rd_ftp::FtpService,
    pub sftp: rd_sftp::SftpService,
    pub object_storage: rd_object_storage::ObjectStorageService,
    pub settings: rd_ftp::SharedRemoteSettings,
}
