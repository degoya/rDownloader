//! SQLite persistence and serialized writer operations.

#![warn(unreachable_pub)]

mod archive_password;
mod auth_flow_store;
mod auth_profile_store;
mod auto_retry_store;
mod automation_store;
mod backup_ledger_store;
mod backup_store;
mod bandwidth_store;
mod capture_store;
mod collector_media;
mod collector_mirrors;
mod collision_store;
mod remote_store;
pub use collector_media::media_file_name;
mod audit_store;
mod collector_packages;
mod collector_store;
mod commands;
mod config_store;
mod download_sources_store;
mod error;
mod event_bus;
mod facade_access;
mod facade_archive_password;
mod facade_audit;
mod facade_automation;
mod facade_bandwidth;
mod facade_candidates;
mod facade_collector;
mod facade_collector_mirrors;
mod facade_credentials;
mod facade_destinations;
mod facade_downloads;
mod facade_full_backup;
mod facade_history;
mod facade_indexers;
mod facade_logs;
mod facade_network;
mod facade_notify;
mod facade_nzb;
mod facade_object_storage;
mod facade_plugin_repositories;
mod facade_plugins;
mod facade_postprocess;
mod facade_resolver;
mod facade_site_rule_checks;
mod facade_site_rule_switches;
mod facade_site_rules;
mod facade_sources;
mod facade_stats;
mod facade_storage;
mod facade_streams;
mod facade_subscriptions;
mod facade_usenet_traffic;
mod full_backup_store;
mod helpers;
mod history_store;
mod indexer_store;
mod json_column;
mod log_store;
mod managed_tools_store;
mod mfa_store;
mod models;
mod network_store;
mod notice_store;
mod notify_store;
mod nzb_hopeless;
mod nzb_queue;
mod nzb_store;
mod object_storage_store;
mod open;
mod package_names;
mod package_relocation_store;
mod package_speed_limit_store;
mod package_store;
mod plugin_execution_store;
mod plugin_keys_store;
mod plugin_repositories_store;
mod plugin_revocations_store;
mod plugin_transfer_store;
mod plugin_versions_store;
mod postprocess_store;
pub mod pre_migration;
mod remote_job_store;
mod replay_store;
pub mod restore_copy;
mod retention;
mod service_settings;
mod session_store;
mod site_rule_checks_store;
mod site_rule_switches_store;
mod site_rules_store;
pub mod snapshot;
mod stats_store;
mod storage_ops_store;
mod stream_schedule_store;
mod stream_store;
mod subscription_store;
mod torrent_store;
mod usenet_store;
mod usenet_traffic_store;
mod vault_sweep;
mod writer;
mod writer_jobs;
mod writer_pins;

#[cfg(test)]
mod archive_password_tests;
#[cfg(test)]
mod pre_migration_tests;
#[cfg(test)]
mod stats_tests;
#[cfg(test)]
mod tests;

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use anyhow::{Context, Result};
use rd_core::{EventEnvelope, EventId};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, mpsc};

pub use audit_store::{
    AuditPruneReport, AuditQuery, AuditRecord, CLEARED_DETAIL_KEY, NewAuditRecord,
};
pub use auth_flow_store::UpsertAuthFlow;
pub use auth_profile_store::{NewAuthProfile, UpdateAuthProfile};
pub use auto_retry_store::{AutoRetryCandidate, RetryCounters};
pub use automation_store::{NewAutomation, NewRun};
pub use backup_ledger_store::{
    BACKUP_VERIFICATIONS_KEPT, BackupArchive, BackupRunDestination, BackupRunDestinationEnd,
    BackupVerification, BackupVerificationOutcome, NewBackupArchive,
};
pub use backup_store::{
    ConfigReplacement, ReplacementAccount, ReplacementAuthProfile, ReplacementIndexer,
    ReplacementProxyProfile, ReplacementStreamChannel, ReplacementSubscription,
    ReplacementUsenetServer,
};
pub use bandwidth_store::{NewBandwidthProfile, NewScheduleWindow};
pub use collector_mirrors::{MIRROR_PREFERENCE_KEY, MirrorDissolve};
pub use collector_packages::{CollectorPackageChange, MoveTarget};
pub use collector_store::NewCollectorBatch;
pub use collision_store::{
    CollisionPolicyLevels, CollisionPolicyRow, CollisionPrompt, ContentIndexEntry,
    NewCollisionPrompt, SCOPE_CATEGORY as COLLISION_SCOPE_CATEGORY,
    SCOPE_PACKAGE as COLLISION_SCOPE_PACKAGE,
};
use commands::{MaintenanceCommand, WriterCommand};
pub use config_store::{
    CategoryPostprocess, NewCategory, NewCategoryRule, NewHotFolder, NewStorageRoot,
};
pub use download_sources_store::ChunkMark;
pub use error::{StoreError, StoreErrorKind, store_kind};
pub use event_bus::{EVENT_BUFFER_BYTES, EVENT_BUFFER_EVENTS, EventBus, Replay};
pub use facade_archive_password::NO_VAULT as ARCHIVE_PASSWORD_NO_VAULT;
pub use full_backup_store::{
    BACKUP_INTERRUPTED, BACKUP_RUNS_KEPT, BackupConfig, BackupConfigUpdate,
    BackupDestinationRecord, BackupKeyRecord, BackupRun, BackupRunOutcome, NewBackupDestination,
    NewBackupRun,
};
pub(crate) use helpers::{
    enum_string, escape_like, page_binds, parse_enum, parse_id, parse_time, timestamp,
};
pub use history_store::{
    DOWNLOAD_FAILED_CODE as HISTORY_DOWNLOAD_FAILED_CODE, HistoryPage, HistoryQuery,
    POSTPROCESS_FAILED_CODE as HISTORY_POSTPROCESS_FAILED_CODE,
    UNPACK_FAILED_CODE as HISTORY_UNPACK_FAILED_CODE,
};
pub use indexer_store::NewIndexer;
pub use log_store::{LogPruneReport, LogQuery, LogRecord, NewLogRecord};
pub use managed_tools_store::{ManagedToolRecord, NewManagedTool, ToolManifestState};
pub use models::{NewDownload, NewPackage, PersistedChunk, TransferMetadata};
pub use models::{NewReplayTemplate, NewSecretFragment};
pub use network_store::{NetworkClientConfig, NewAccount, NewProxyProfile, UpdateAccount};
pub use notify_store::{NewDelivery, NewNotificationRule, NewNotificationTarget};
pub use nzb_hopeless::{USENET_AWAITING_PAR2, USENET_JOB_HOPELESS};
pub use nzb_store::{FailedNzbImport, NewNzbFile, NewNzbImport, NewNzbSegment, NzbImportChange};
pub use object_storage_store::{NewObjectStorageProfile, ObjectUpload, ObjectUploadPart};
pub use package_names::{PACKAGE_NAME_REGEX_FIELD, PACKAGE_NAME_RULES_FIELD};
pub use package_store::{CategoryAssignment, PackageChange};
pub use plugin_execution_store::{MAX_EXECUTIONS_PER_PLUGIN, NewPluginExecution, PluginExecution};
pub use plugin_keys_store::{NewPluginTrustedKey, PluginTrustedKey};
pub use plugin_repositories_store::{
    NewPluginRepository, OFFICIAL_REPOSITORY_ID, PluginRepository, PluginRepositoryInstall,
    PluginWithdrawnKey, RepositoryCheck,
};
pub use plugin_revocations_store::{NewPluginDigestRevocation, PluginDigestRevocation};
pub use plugin_transfer_store::PluginTransfer;
pub use plugin_versions_store::{NewPluginVersionChoice, PluginVersionChoice};
pub use postprocess_store::AssembledSegment;
pub use remote_job_store::{AdvanceRemoteJob, ClaimRemoteJob};
pub use remote_store::{HostKeyVerdict, NewRemoteCredential, UpdateRemoteCredential};
pub use replay_store::{REFRESH_WINDOW_HOURS, REPLAY_REFRESH_MAX};
pub use retention::PruneReport;
pub use service_settings::{
    SERVICE_SETTINGS_KEY, SettingsFieldError, parse_service_settings,
    parse_service_settings_per_field, service_setting_field_of,
};
pub use session_store::TOUCH_INTERVAL_SECONDS as SESSION_TOUCH_INTERVAL_SECONDS;
pub use site_rule_checks_store::{NewSiteRuleCheck, SiteRuleCheck};
pub use site_rule_switches_store::{SCOPE_GROUP, SiteRuleSwitch};
pub use site_rules_store::{NewUserSiteRule, UserSiteRule};
pub use stats_store::{
    DIRECT_PROVIDER, PRUNE_BATCH, StatsPruneReport, StatsResolution, StatsRetention,
    TransferBucket, TransferTotal,
};
pub use storage_ops_store::{
    NewStorageOperation, STORAGE_OPERATIONS_KEPT, StorageOperation, StorageOperationOutcome,
};
pub use stream_schedule_store::{NewStreamSchedule, PlannedOccurrence};
pub use stream_store::NewStreamChannel;
pub use subscription_store::{NewSubscription, NewSubscriptionItem, PollResult};
pub use usenet_store::{
    NewUsenetServer, UpdateUsenetServer, UsenetConnectionConfig, UsenetQuotaInput,
};
pub use usenet_traffic_store::{UsenetQuotaReached, UsenetServerTraffic};

/// SQLite database facade with one serialized writer and a small reader pool.
#[derive(Clone)]
pub struct Database {
    readers: SqlitePool,
    writer: mpsc::Sender<WriterCommand>,
    events: EventBus,
    /// The vault a declaring provider's link fragment is put away in (RD-110-38).
    ///
    /// Installed after the database is opened rather than passed to [`Database::open`]: the
    /// binary opens the store first so the diagnostic log sink has somewhere to write, and
    /// the vault's master key comes from the keyring one step later. A `OnceLock` behind an
    /// `Arc` so every clone made in between sees it once it arrives, and so it can never be
    /// swapped for a second vault whose key would not open what the first one wrote.
    vault: Arc<OnceLock<rd_secrets::SecretStore>>,
    /// The database file, whose folder is the data directory a full backup stages in.
    path: Arc<PathBuf>,
}

/// The migrations this build carries, compiled in from `migrations/`.
pub(crate) static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

impl Database {
    /// Subscribes to committed domain events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    /// Subscribes and, in the same step, hands back every event buffered after `after` --
    /// or [`Replay::Expired`] when the buffer no longer reaches that far (RD-110-23). The
    /// buffer is in memory: after a restart every id is expired.
    #[must_use]
    pub fn resume(&self, after: EventId) -> (Replay, broadcast::Receiver<EventEnvelope>) {
        self.events.resume(after)
    }

    /// Publishes an event to live subscribers without persisting it. Only for state that
    /// exists solely while the process runs, such as a captcha waiting to be solved:
    /// replaying it from the event log after a restart would be meaningless.
    pub fn broadcast(&self, event: EventEnvelope) {
        let _ = self.events.send(event);
    }

    /// Persists a JSON setting.
    pub async fn set_setting(&self, key: String, value: serde_json::Value) -> Result<()> {
        writer::request(&self.writer, |reply| MaintenanceCommand::SetSetting {
            key,
            value,
            reply,
        })
        .await
    }

    /// Writes a JSON setting only if the key holds nothing yet, and says whether it did.
    ///
    /// For a value that may be set exactly once, such as the first administrator password: a
    /// read followed by [`Self::set_setting`] lets two requests both find the key empty.
    pub async fn insert_setting_if_absent(
        &self,
        key: String,
        value: serde_json::Value,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            MaintenanceCommand::InsertSettingIfAbsent { key, value, reply }
        })
        .await
    }

    /// Reads a JSON setting.
    pub async fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let raw = sqlx::query_scalar::<_, String>("SELECT value_json FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.readers)
            .await?;
        raw.map(|value| serde_json::from_str(&value).context("decode setting"))
            .transpose()
    }
}
