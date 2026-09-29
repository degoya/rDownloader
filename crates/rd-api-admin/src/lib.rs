//! Configuration and upkeep (RD-160-06): categories, accounts, hot folders and proxies, the
//! settings document and its backups, the scheduled full backup (RD-160-01) with its
//! destinations, retention and verification (RD-160-02) and its restore (RD-160-03), plugins and
//! their repositories, managed tools, notifications and automations, remote and object storage
//! profiles, diagnostics, statistics and the head of the About page.

pub mod about_page;
pub mod automation_handlers;
pub mod backup_delivery;
pub mod backup_destination_handlers;
pub mod backup_handlers;
pub mod backup_service;
pub mod backup_verify_service;
pub mod config_handlers;
pub mod data_reset_handlers;
pub mod diagnostics_checks;
pub mod diagnostics_dto;
pub mod diagnostics_handlers;
pub mod notify_handlers;
pub mod object_storage_handlers;
pub mod plugin_bundled;
pub mod plugin_handlers;
pub mod plugin_lifecycle;
mod plugin_live;
pub mod plugin_repository_dto;
pub mod plugin_repository_handlers;
pub mod plugin_update_policy;
mod protected_roots;
pub mod providers_handlers;
pub mod remote_handlers;
mod restore_checks;
pub mod restore_dto;
pub mod restore_handlers;
pub mod restore_service;
pub mod restore_uploads;
pub mod routing_backup;
pub mod settings_backup;
pub mod settings_backup_auth;
pub mod settings_backup_crypto;
pub mod settings_backup_dto;
pub mod settings_backup_secrets;
pub mod settings_handlers;
pub mod stats_handlers;
pub mod stats_retention_service;
pub mod tools_handlers;

// The modules of the crates below, at this crate's root, so that a module here names them as
// `crate::…` exactly as it did while the HTTP surface was one crate (RD-160-06).
use rd_api_core::{
    ApiError, AppState, BuildInfo, audit, auth, automation_input, automation_service,
    config_fields, dto, error, error_codes, host_check, hosters, hotfolder_service, notify_service,
    postprocess_handlers, settings_store,
};
