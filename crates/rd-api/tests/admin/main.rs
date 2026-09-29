//! rd-api integration tests: running the installation: backups, reset, diagnostics, metrics, tools, plugins, site rules, notifications and automations.
//!
//! One test binary per subject, each suite a module of it (RD-150-10). Every binary links the
//! whole service, and one binary per file meant 57 links of ~550 MB each. A new suite is a
//! module here and a row in `scripts/lib/rd-api-tests.map`, which selects suites by these
//! module names.

#[path = "../common/mod.rs"]
mod common;

mod area_backup;
mod automation_triggers;
mod automations;
mod backup_destinations;
mod backup_remote;
mod data_reset;
mod diagnostics;
mod full_backup;
mod full_restore;
mod managed_tools;
mod metrics;
mod notifications;
mod plugin_auto_updates;
mod plugin_bundled;
mod plugin_enabled;
mod plugin_repositories;
mod plugin_versions;
mod protected_roots;
mod routing_backup;
mod settings_backup;
mod site_rules;
