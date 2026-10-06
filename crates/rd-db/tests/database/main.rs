//! rd-db integration tests: the stores, the migrations and the backups, against a real database.
//!
//! One test binary, each former test file a module of it (RD-1120-08, the RD-150-10 pattern of
//! `rd-api`): 31 binaries linked the same crate for tests that open a database each. The two
//! crash binaries, `archive_password_crash` and `download_history_crash`, stay alone beside it —
//! `scripts/lib/crash-matrix.list` runs them by name with the `failpoints` feature on.

mod audit_store;
mod backup_ledger;
mod category_defaults;
mod collisions;
mod config_events;
mod data_reset;
mod download_history;
mod download_sources;
mod event_retention;
mod full_backup;
mod indexers;
mod log_store;
mod migration_checksums;
mod migration_forward;
mod nzb_history;
mod oauth_flows;
mod object_storage;
mod plugin_repositories;
mod remote_jobs;
mod secret_fragment;
mod sessions;
mod settings_backup;
mod settings_import_children;
mod site_rule_checks;
mod site_rules;
mod storage_root_defaults;
mod subscription_events;
mod subscription_filters;
mod subscription_git_release;
mod transform_key;
mod vault_references;
