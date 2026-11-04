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
pub mod automation_links;
mod automation_schedule;
pub mod automation_service;
#[cfg(test)]
mod automation_test_support;
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
pub mod download_window_input;
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
pub mod links_file;
pub mod links_nzb;
pub mod list_bounds;
pub mod local_control;
pub mod malware_scanner_handlers;
pub mod notify_notice;
pub mod notify_service;
pub mod nzb_candidate;
pub mod object_upload_target;
pub mod oidc_client;
pub mod password_reset;
pub mod postprocess_handlers;
pub mod power_service;
pub mod qbittorrent_sessions;
mod queue_state;
pub mod reconnect_decision;
pub mod reconnect_ip;
pub mod reconnect_service;
pub mod remote_job_service;
pub mod restart_state;
pub mod scope_policy;
pub mod settings_store;
pub mod step_up;
pub mod storage_capacity;
pub mod stream_monitor;
pub mod stream_standing;
pub mod subscription_hosts;
pub mod subscription_service;
pub mod torrent_candidate;
pub mod torrent_intake;
pub mod trace_context;
pub mod update_service;
pub mod web_push;

use rd_db::Database;
use rd_scheduler::SchedulerHandle;

pub use app_state::{
    AccessState, AdminState, AppState, AppStateBuilder, IntakeState, QueueState, RuntimeState,
};
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

/// The milestone 0.6 transfer services, grouped so they travel as one argument.
#[derive(Clone)]
pub struct RemoteServices {
    pub ftp: rd_ftp::FtpService,
    pub sftp: rd_sftp::SftpService,
    pub object_storage: rd_object_storage::ObjectStorageService,
    pub settings: rd_ftp::SharedRemoteSettings,
}
