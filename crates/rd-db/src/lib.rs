//! SQLite persistence and serialized writer operations.

mod auth_flow_store;
mod auth_profile_store;
mod automation_store;
mod backup_store;
mod bandwidth_store;
mod capture_store;
mod collector_media;
mod collector_mirrors;
mod remote_store;
pub use collector_media::media_file_name;
mod audit_store;
mod collector_packages;
mod collector_store;
mod commands;
mod config_store;
mod error;
mod event_bus;
mod facade_audit;
mod facade_collector;
mod facade_ext;
mod facade_logs;
mod facade_site_rule_checks;
mod facade_site_rule_switches;
mod facade_site_rules;
mod facade_stats;
mod log_store;
mod managed_tools_store;
mod mfa_store;
mod models;
mod network_store;
mod notify_store;
mod nzb_queue;
mod nzb_store;
mod package_store;
mod plugin_execution_store;
mod plugin_keys_store;
mod plugin_revocations_store;
mod plugin_transfer_store;
mod postprocess_store;
mod remote_job_store;
mod replay_store;
mod service_settings;
mod session_store;
mod site_rule_checks_store;
mod site_rule_switches_store;
mod site_rules_store;
mod stats_store;
mod stream_schedule_store;
mod stream_store;
mod subscription_store;
mod torrent_store;
mod usenet_store;
mod writer;
mod writer_jobs;

#[cfg(test)]
mod stats_tests;
#[cfg(test)]
mod tests;

use std::{
    path::Path,
    str::FromStr,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result};
use rd_core::{DownloadFile, DownloadId, DownloadPackage, EventEnvelope, EventId};
use sqlx::{
    ConnectOptions, Connection, SqliteConnection, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use tokio::sync::{broadcast, mpsc};
use tracing::log::LevelFilter;

pub use audit_store::{
    AuditPruneReport, AuditQuery, AuditRecord, CLEARED_DETAIL_KEY, NewAuditRecord,
};
pub use auth_flow_store::UpsertAuthFlow;
pub use auth_profile_store::{NewAuthProfile, UpdateAuthProfile};
pub use automation_store::{NewAutomation, NewRun};
pub use backup_store::{
    ConfigReplacement, ReplacementAccount, ReplacementAuthProfile, ReplacementProxyProfile,
    ReplacementStreamChannel, ReplacementSubscription, ReplacementUsenetServer,
};
pub use bandwidth_store::{NewBandwidthProfile, NewScheduleWindow};
pub use collector_mirrors::{MIRROR_PREFERENCE_KEY, MirrorDissolve};
pub use collector_packages::{CollectorPackageChange, MoveTarget};
pub use collector_store::NewCollectorBatch;
use commands::WriterCommand;
pub use config_store::{
    CategoryPostprocess, NewCategory, NewCategoryRule, NewHotFolder, NewStorageRoot,
};
pub use error::{StoreError, StoreErrorKind, store_kind};
pub use event_bus::{EVENT_BUFFER_BYTES, EVENT_BUFFER_EVENTS, EventBus, Replay};
pub use log_store::{LogPruneReport, LogQuery, LogRecord, NewLogRecord};
pub use managed_tools_store::{ManagedToolRecord, NewManagedTool, ToolManifestState};
pub use models::{NewDownload, NewPackage, PersistedChunk, TransferMetadata};
pub use models::{NewReplayTemplate, NewSecretFragment};
pub use network_store::{NetworkClientConfig, NewAccount, NewProxyProfile, UpdateAccount};
pub use notify_store::{NewDelivery, NewNotificationRule, NewNotificationTarget};
pub use nzb_store::{FailedNzbImport, NewNzbFile, NewNzbImport, NewNzbSegment, NzbImportChange};
pub use package_store::{CategoryAssignment, PackageChange};
pub use plugin_execution_store::{MAX_EXECUTIONS_PER_PLUGIN, NewPluginExecution, PluginExecution};
pub use plugin_keys_store::{NewPluginTrustedKey, PluginTrustedKey};
pub use plugin_revocations_store::{NewPluginDigestRevocation, PluginDigestRevocation};
pub use plugin_transfer_store::PluginTransfer;
pub use remote_job_store::{AdvanceRemoteJob, ClaimRemoteJob};
pub use remote_store::{HostKeyVerdict, NewRemoteCredential, UpdateRemoteCredential};
pub use replay_store::{REFRESH_WINDOW_HOURS, REPLAY_REFRESH_MAX};
pub use service_settings::{
    SERVICE_SETTINGS_KEY, parse_service_settings, service_setting_field_of,
};
pub use session_store::TOUCH_INTERVAL_SECONDS as SESSION_TOUCH_INTERVAL_SECONDS;
pub use site_rule_checks_store::{NewSiteRuleCheck, SiteRuleCheck};
pub use site_rule_switches_store::{SCOPE_GROUP, SCOPE_RULE, SiteRuleSwitch};
pub use site_rules_store::{NewUserSiteRule, UserSiteRule};
pub use stats_store::{
    DIRECT_PROVIDER, PRUNE_BATCH, StatsPruneReport, StatsResolution, StatsRetention,
    TransferBucket, TransferTotal,
};
pub use stream_schedule_store::{NewStreamSchedule, PlannedOccurrence};
pub use stream_store::NewStreamChannel;
pub use subscription_store::{NewSubscription, NewSubscriptionItem, PollResult};
pub use usenet_store::{NewUsenetServer, UpdateUsenetServer, UsenetConnectionConfig};
use writer::Writer;

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
}

/// Every unfinished download bound to one plugin id and version, by download id.
///
/// The two records that name a version: the resolver pin a job claimed, and the checkpoint a
/// transfer backend wrote. Takes the id and the version twice, once per branch.
const PLUGIN_VERSION_BINDINGS: &str = "\
    SELECT pin.download_id AS download_id FROM download_resolver_pins pin \
      JOIN downloads job ON job.id = pin.download_id \
     WHERE pin.plugin_id = ? AND pin.plugin_version = ? AND job.state != 'completed' \
    UNION \
    SELECT transfer.download_id AS download_id FROM plugin_transfers transfer \
      JOIN downloads job ON job.id = transfer.download_id \
     WHERE transfer.plugin_id = ? AND transfer.plugin_version = ? AND job.state != 'completed'";

impl Database {
    /// Opens a database file, applies migrations and starts the writer actor.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .log_statements(LevelFilter::Trace);

        let mut writer_connection = SqliteConnection::connect_with(&options)
            .await
            .context("open SQLite writer connection")?;
        sqlx::migrate!()
            .run(&mut writer_connection)
            .await
            .context("apply SQLite migrations")?;

        let readers = SqlitePoolOptions::new()
            .max_connections(4)
            .min_connections(1)
            .connect_with(options)
            .await
            .context("open SQLite reader pool")?;

        let events = EventBus::new();
        let (command_tx, command_rx) = mpsc::channel(256);
        tokio::spawn(Writer::new(writer_connection, command_rx, events.clone()).run());

        Ok(Self {
            readers,
            writer: command_tx,
            events,
            vault: Arc::new(OnceLock::new()),
        })
    }

    /// Hands the database the vault it puts secret link fragments in (RD-110-38).
    ///
    /// Without it intake behaves exactly as it did before: a fragment is dropped, whatever
    /// the provider declared. That is the right degradation -- a key nothing can read back is
    /// worse than no key -- and it is what `rdownloader doctor` and the plugin CLI run with.
    pub fn install_secret_vault(&self, vault: rd_secrets::SecretStore) {
        let _ = self.vault.set(vault);
    }

    /// The installed vault, if there is one.
    #[must_use]
    pub fn secret_vault(&self) -> Option<&rd_secrets::SecretStore> {
        self.vault.get()
    }

    /// Removes vaulted material whose owning row is gone. Never fails an operation: the row
    /// is already deleted, and a secret that could not be removed is a warning, not a reason
    /// to report the deletion as failed.
    pub(crate) async fn forget_secrets(&self, references: Vec<String>) {
        let Some(vault) = self.vault.get() else {
            return;
        };
        for reference in references {
            if let Err(error) = vault.remove(&reference).await {
                tracing::warn!(%error, "a vaulted link fragment could not be removed");
            }
        }
    }

    /// Atomically replaces every server-side configuration table while preserving supplied ids.
    pub async fn replace_config(&self, replacement: ConfigReplacement) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ReplaceConfig {
            replacement,
            reply,
        })
        .await
    }

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

    /// Creates an empty package.
    pub async fn create_package(&self, package: NewPackage) -> Result<DownloadPackage> {
        writer::request(&self.writer, |reply| WriterCommand::CreatePackage {
            package,
            reply,
        })
        .await
    }

    /// Adds a file to a package and creates its initial chunk.
    pub async fn create_download(&self, download: NewDownload) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| WriterCommand::CreateDownload {
            download,
            reply,
        })
        .await
    }

    /// Changes a file state and commits a matching event atomically.
    pub async fn transition_download(
        &self,
        id: DownloadId,
        next: rd_core::DownloadState,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| WriterCommand::TransitionDownload {
            id,
            next,
            reply,
        })
        .await
    }

    /// Blocks a file and records why, so the release path can tell the causes apart.
    ///
    /// `Blocked` is one state for several unrelated causes and the releases are not
    /// interchangeable — freeing disk space must not restart a transfer whose validators
    /// changed mid-flight. The vocabulary of reasons belongs to the caller; this layer only
    /// stores the string and hands it back through [`Self::downloads_blocked_by`].
    pub async fn block_download(&self, id: DownloadId, reason: &str) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| WriterCommand::BlockDownload {
            id,
            reason: reason.to_owned(),
            reply,
        })
        .await
    }

    /// Ids of the blocked files that were blocked for `reason`.
    ///
    /// Ids only: the caller already holds the rows it cares about, and this read exists to
    /// narrow a release down to one cause, not to load the table a second time.
    pub async fn downloads_blocked_by(&self, reason: &str) -> Result<Vec<DownloadId>> {
        models::downloads_blocked_by(&self.readers, reason).await
    }

    /// Removes an inactive queue entry and its now-empty package metadata.
    pub async fn delete_download(&self, id: DownloadId) -> Result<()> {
        // The second owner of a vaulted link fragment (RD-110-38). Read before the delete for
        // the same reason the candidate's is: the reference is a column of the row going away.
        let orphaned = self.download_secret_fragment_ref(id).await.unwrap_or(None);
        // The third owner of vaulted material on this row (RD-120-11): the transform key.
        // Read before the delete for the same reason, and forgotten with the fragment.
        let key_reference = self.download_transform_key_ref(id).await.unwrap_or(None);
        writer::request(&self.writer, |reply| WriterCommand::DeleteDownload {
            id,
            reply,
        })
        .await?;
        self.forget_secrets(orphaned.into_iter().chain(key_reference).collect())
            .await;
        Ok(())
    }

    /// The reference a download row holds, without opening the vault.
    async fn download_secret_fragment_ref(&self, id: DownloadId) -> Result<Option<String>> {
        use sqlx::Row;

        let row = sqlx::query("SELECT secret_fragment_ref FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.readers)
            .await?;
        Ok(row.and_then(|row| {
            row.try_get::<Option<String>, _>("secret_fragment_ref")
                .ok()
                .flatten()
        }))
    }

    /// Removes a package that has no files at all; returns whether one was removed.
    ///
    /// The package-level counterpart of [`Self::delete_download`], which can only drop a
    /// package as a side effect of removing its last file. A half-written package whose very
    /// first file failed has no such file, and the empty row it left behind reads in the queue
    /// exactly like a package that downloaded nothing. A package that still has files is not
    /// touched, so a rollback may call this unconditionally.
    pub async fn delete_empty_package(&self, id: rd_core::PackageId) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteEmptyPackage {
            id,
            reply,
        })
        .await
    }

    /// Persists a post-sync checkpoint for a chunk.
    /// Progress of a runner-driven file (media): bytes so far and, when known, the total.
    pub async fn set_download_progress(
        &self,
        id: DownloadId,
        committed_bytes: u64,
        total_bytes: Option<u64>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetDownloadProgress {
            id,
            committed_bytes,
            total_bytes,
            reply,
        })
        .await
    }

    /// Replaces the seeding override of one category; `None` clears it.
    pub async fn set_category_seeding_policy(
        &self,
        id: rd_core::CategoryId,
        policy: Option<rd_core::SeedingPolicyOverride>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::SetCategorySeedingPolicy { id, policy, reply }
        })
        .await
    }

    /// Replaces the torrent state of one link candidate (file tree, plan, metadata).
    pub async fn set_candidate_torrent_state(
        &self,
        id: rd_core::CandidateId,
        state: rd_core::TorrentCandidateState,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::SetCandidateTorrentState {
                id,
                state: Box::new(state),
                reply,
            }
        })
        .await
    }

    /// Replaces the torrent state of one queue row (plan, trackers, seeding, accounting).
    pub async fn set_download_torrent_state(
        &self,
        id: DownloadId,
        state: rd_core::TorrentJobState,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::SetDownloadTorrentState {
                id,
                state: Box::new(state),
                reply,
            }
        })
        .await
    }

    /// Torrent state of one link candidate.
    pub async fn candidate_torrent_state(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::TorrentCandidateState>> {
        let mut connection = self.readers.acquire().await?;
        torrent_store::candidate_state(&mut connection, id).await
    }

    /// Torrent state of one queue row.
    pub async fn download_torrent_state(
        &self,
        id: DownloadId,
    ) -> Result<Option<rd_core::TorrentJobState>> {
        let mut connection = self.readers.acquire().await?;
        torrent_store::download_state(&mut connection, id).await
    }

    /// Torrent state of every queue row that has one, for restart recovery.
    pub async fn all_download_torrent_states(
        &self,
    ) -> Result<Vec<(DownloadId, rd_core::TorrentJobState)>> {
        let mut connection = self.readers.acquire().await?;
        torrent_store::all_download_states(&mut connection).await
    }

    pub async fn checkpoint_chunk(
        &self,
        chunk_id: rd_core::ChunkId,
        committed_offset: u64,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointChunk {
            chunk_id,
            committed_offset,
            reply,
        })
        .await
    }

    /// Records one finished provider-chunk MAC of a transformed stream (RD-103-02).
    pub async fn checkpoint_chunk_mac(
        &self,
        download_id: DownloadId,
        fingerprint: String,
        index: u64,
        mac: [u8; 16],
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointChunkMac {
            download_id,
            fingerprint,
            index,
            mac,
            reply,
        })
        .await
    }

    /// What a previous attempt at this file finished, for the transform to adopt.
    ///
    /// Empty for every ordinary download, which has no transform and therefore no MACs.
    pub async fn transform_checkpoint(
        &self,
        download_id: DownloadId,
    ) -> Result<(Option<String>, Vec<(usize, [u8; 16])>)> {
        let mut connection = self.readers.acquire().await?;
        models::transform_checkpoint(&mut connection, download_id).await
    }

    /// Stores validators and a newly planned set of chunks before downloading starts.
    pub async fn prepare_transfer(
        &self,
        id: DownloadId,
        total_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        chunks: Vec<PersistedChunk>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::PrepareTransfer {
            id,
            total_bytes,
            etag,
            last_modified,
            chunks,
            reply,
        })
        .await
    }

    /// Loads crash-safe range and validator metadata.
    pub async fn load_transfer(&self, id: DownloadId) -> Result<TransferMetadata> {
        models::load_transfer(&self.readers, id).await
    }

    /// Persists a scheduler failure, retry time and resulting state.
    pub async fn record_failure(
        &self,
        id: DownloadId,
        failure: rd_core::Failure,
        retry_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| WriterCommand::RecordFailure {
            id,
            failure,
            retry_at,
            reply,
        })
        .await
    }

    /// Marks a verified file complete and stores its optional generated checksum.
    pub async fn complete_download(
        &self,
        id: DownloadId,
        final_name: String,
        checksum: Option<rd_core::ExpectedChecksum>,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| WriterCommand::CompleteDownload {
            id,
            final_name,
            checksum,
            reply,
        })
        .await
    }

    /// Persists a collision-resolved filename before file IO starts.
    pub async fn set_download_file_name(&self, id: DownloadId, file_name: String) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetFileName {
            id,
            file_name,
            reply,
        })
        .await
    }

    /// Renames a file that is not active or finished; returns the updated row.
    pub async fn rename_download(&self, id: DownloadId, file_name: String) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| WriterCommand::RenameDownload {
            id,
            file_name,
            reply,
        })
        .await
    }

    /// Consented replay template of a download, if it has one.
    pub async fn request_template(
        &self,
        id: DownloadId,
    ) -> Result<Option<rd_core::RequestTemplate>> {
        replay_store::request_template(&self.readers, id).await
    }

    /// `vault://` reference of a download's request body.
    pub async fn request_body_ref(&self, id: DownloadId) -> Result<Option<String>> {
        replay_store::template_body_ref(&self.readers, id).await
    }

    /// `vault://` reference of a candidate's captured request body.
    pub async fn candidate_body_ref(&self, id: rd_core::CandidateId) -> Result<Option<String>> {
        replay_store::candidate_body_ref(&self.readers, id).await
    }

    /// Replay consent recorded for a candidate.
    pub async fn candidate_replay_consent(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::ReplayConsent>> {
        replay_store::candidate_consent(&self.readers, id).await
    }

    /// Records or withdraws a candidate's replay consent.
    pub async fn set_candidate_replay_consent(
        &self,
        id: rd_core::CandidateId,
        consent: Option<rd_core::ReplayConsent>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::SetCandidateReplayConsent {
                id,
                consent: Box::new(consent),
                reply,
            }
        })
        .await
    }

    /// Atomically reserves one pre-resume replay refresh from the windowed budget.
    ///
    /// Separate from [`Self::claim_resolver_refresh`] on purpose; see `replay_store`.
    pub async fn claim_replay_refresh(&self, id: DownloadId) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::ClaimReplayRefresh {
            id,
            reply,
        })
        .await
    }

    /// Puts a download back to `queued` with everything it produced discarded.
    ///
    /// Unlike `reset_transfer` this is the whole job: the retry budget, the recorded error, the
    /// Usenet segment checkpoints and the package's post-processing steps go with it.
    pub async fn reset_download(&self, id: DownloadId) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| WriterCommand::ResetDownload {
            id,
            reply,
        })
        .await
    }

    /// Discards a download's partial state so a refreshed URL starts from zero.
    pub async fn reset_transfer(&self, id: DownloadId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ResetTransfer {
            id,
            reply,
        })
        .await
    }

    /// Every vaulted request body whose owning candidate or download is gone.
    pub async fn orphaned_replay_body_refs(&self) -> Result<Vec<String>> {
        replay_store::orphaned_body_refs(&self.readers).await
    }

    /// Atomically reserves the single resolver refresh allowed after HTTP 401/403.
    pub async fn claim_resolver_refresh(&self, id: DownloadId) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::ClaimResolverRefresh {
            id,
            reply,
        })
        .await
    }

    /// Returns the exact resolver version already assigned to a download.
    pub async fn resolver_pin(&self, id: DownloadId) -> Result<Option<rd_core::ResolverPin>> {
        use sqlx::Row;

        let row = sqlx::query(
            "SELECT plugin_id, plugin_version FROM download_resolver_pins WHERE download_id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.readers)
        .await?;
        row.map(|row| {
            Ok(rd_core::ResolverPin {
                plugin_id: parse_id(row.get::<String, _>("plugin_id").as_str())?,
                version: row.get("plugin_version"),
            })
        })
        .transpose()
    }

    /// Atomically writes the first resolver pin and returns the winner of any race.
    pub async fn claim_resolver_pin(
        &self,
        id: DownloadId,
        pin: rd_core::ResolverPin,
    ) -> Result<rd_core::ResolverPin> {
        writer::request(&self.writer, |reply| WriterCommand::ClaimResolverPin {
            id,
            pin,
            reply,
        })
        .await
    }

    /// The newest recorded invocations of one plugin, newest first.
    pub async fn plugin_executions(
        &self,
        plugin_id: &str,
        limit: i64,
    ) -> Result<Vec<PluginExecution>> {
        plugin_execution_store::list_plugin_executions(&self.readers, plugin_id, limit).await
    }

    /// How many recorded invocations each plugin has, for the plugins that have any.
    ///
    /// One grouped read, so the plugin manager can tell a plugin that never ran from one that
    /// ran without incident without fetching a single entry.
    pub async fn plugin_execution_counts(&self) -> Result<Vec<(String, i64)>> {
        plugin_execution_store::plugin_execution_counts(&self.readers).await
    }

    /// Records one invocation and trims the plugin's history to its cap.
    ///
    /// Diagnostics are optional by definition: a caller that cannot write one carries on with
    /// the download rather than failing it.
    pub async fn record_plugin_execution(&self, entry: NewPluginExecution) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::RecordPluginExecution {
            entry: Box::new(entry),
            reply,
        })
        .await
    }

    /// Resume state of a plugin transfer, if the job has one.
    pub async fn plugin_transfer(&self, id: DownloadId) -> Result<Option<PluginTransfer>> {
        plugin_transfer_store::load_plugin_transfer(&self.readers, id).await
    }

    /// Persists a transfer backend's checkpoint, claiming the version pin on first write.
    pub async fn save_plugin_transfer(
        &self,
        id: DownloadId,
        plugin_id: String,
        plugin_version: String,
        checkpoint: Option<Vec<u8>>,
    ) -> Result<PluginTransfer> {
        writer::request(&self.writer, |reply| WriterCommand::SavePluginTransfer {
            id,
            plugin_id,
            plugin_version,
            checkpoint,
            reply,
        })
        .await
    }

    /// Forgets a transfer's resume state once it finished or was discarded.
    pub async fn clear_plugin_transfer(&self, id: DownloadId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ClearPluginTransfer {
            id,
            reply,
        })
        .await
    }

    /// How many unfinished downloads are bound to exactly this plugin id and version.
    ///
    /// Bound means a record decides how the job continues, and names that one version: the
    /// resolver pin a job claimed, or the checkpoint a transfer backend wrote. Both are
    /// durable on purpose -- a paused job resumes with the version that started it -- which is
    /// why the in-memory leases `rd-tools` keeps for external binaries do not transfer here.
    /// A restart forgets a lease; it must not forget that a paused job still needs one
    /// version out of two installed ones.
    ///
    /// Only a `completed` download is finished for this purpose. Everything else can still be
    /// started, resumed or retried by hand -- a cancelled job included: `ProgressControl::cancel`
    /// keeps the partial data and `resume` puts it back in the queue, so its checkpoint is a
    /// claim like any other. A job that is really gone is deleted, and deleting it takes its
    /// pin and its checkpoint with it.
    pub async fn plugin_version_usage(&self, plugin_id: &str, version: &str) -> Result<u64> {
        use sqlx::Row;

        let row = sqlx::query(&format!(
            "SELECT COUNT(*) AS bound FROM ({PLUGIN_VERSION_BINDINGS})"
        ))
        .bind(plugin_id)
        .bind(version)
        .bind(plugin_id)
        .bind(version)
        .fetch_one(&self.readers)
        .await?;
        Ok(u64::try_from(row.get::<i64, _>("bound")).unwrap_or(0))
    }

    /// Names the first `limit` downloads that hold this plugin version, oldest first.
    ///
    /// A refusal that only says how many jobs are in the way leaves the reader looking for
    /// them; the file names are what makes the blocker findable in the queue.
    pub async fn plugin_version_blockers(
        &self,
        plugin_id: &str,
        version: &str,
        limit: i64,
    ) -> Result<Vec<String>> {
        use sqlx::Row;

        let rows = sqlx::query(&format!(
            "SELECT job.file_name AS file_name FROM ({PLUGIN_VERSION_BINDINGS}) binding \
               JOIN downloads job ON job.id = binding.download_id \
              ORDER BY job.created_at LIMIT ?"
        ))
        .bind(plugin_id)
        .bind(version)
        .bind(plugin_id)
        .bind(version)
        .bind(limit)
        .fetch_all(&self.readers)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| row.get::<String, _>("file_name"))
            .collect())
    }

    /// Drops resolver pins that name a version this build can no longer provide.
    ///
    /// Returns how many jobs were freed. See the writer implementation for why an
    /// unsatisfiable pin is worse than no pin at all.
    pub async fn clear_unsatisfiable_resolver_pins(
        &self,
        available: Vec<(String, String)>,
    ) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ClearUnsatisfiableResolverPins { available, reply }
        })
        .await
    }

    /// Returns packages in queue order.
    pub async fn list_packages(&self) -> Result<Vec<DownloadPackage>> {
        models::list_packages(&self.readers).await
    }

    /// Returns files in creation order.
    pub async fn list_downloads(&self) -> Result<Vec<DownloadFile>> {
        models::list_downloads(&self.readers).await
    }

    /// Returns one package's files in queue order.
    ///
    /// Callers that only care about one package must use this instead of filtering
    /// [`Self::list_downloads`]: that read loads the whole table, JSON blob columns
    /// included, and the completion check runs it once per finished download.
    pub async fn downloads_for_package(
        &self,
        package_id: rd_core::PackageId,
    ) -> Result<Vec<DownloadFile>> {
        models::downloads_for_package(&self.readers, package_id).await
    }

    /// Loads one file.
    pub async fn get_download(&self, id: DownloadId) -> Result<Option<DownloadFile>> {
        models::get_download(&self.readers, id).await
    }

    /// Resets interrupted active states to queued during startup recovery.
    pub async fn recover_interrupted(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::RecoverInterrupted {
            reply,
        })
        .await
    }

    /// Checkpoints the WAL after all writer commands already sent have completed.
    pub async fn checkpoint_wal(&self) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointWal { reply }).await
    }

    /// Creates account metadata referring to separately stored secrets.
    pub async fn create_account(&self, input: NewAccount) -> Result<rd_core::Account> {
        writer::request(&self.writer, |reply| WriterCommand::CreateAccount {
            input,
            reply,
        })
        .await
    }

    /// Updates public account metadata and its selected secret references.
    pub async fn update_account(
        &self,
        id: rd_core::AccountId,
        input: UpdateAccount,
    ) -> Result<rd_core::Account> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateAccount {
            id,
            input,
            reply,
        })
        .await
    }

    /// Deletes an unused provider account and returns its opaque secret references for cleanup.
    pub async fn delete_account(
        &self,
        id: rd_core::AccountId,
    ) -> Result<(Option<String>, Option<String>)> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteAccount {
            id,
            reply,
        })
        .await
    }

    /// Loads opaque account secret references for replacement without exposing values.
    pub async fn account_secret_refs(
        &self,
        id: rd_core::AccountId,
    ) -> Result<Option<(Option<String>, Option<String>)>> {
        network_store::account_secret_refs(&self.readers, id).await
    }

    /// Creates or advances the authentication flow of one account (RD-090-13).
    pub async fn upsert_auth_flow(
        &self,
        input: crate::auth_flow_store::UpsertAuthFlow,
    ) -> Result<rd_core::AuthFlow> {
        writer::request(&self.writer, |reply| WriterCommand::UpsertAuthFlow {
            input: Box::new(input),
            reply,
        })
        .await
    }

    /// Records what a renewal produced: the expiry, the refresh reference (RD-103-00) and,
    /// for a provider that keeps its access token here rather than on the account, that
    /// reference too (RD-106-03).
    ///
    /// `access_ref` of `None` leaves the stored one alone. Every provider but the one shape
    /// passes `None` for ever, and clearing it on each renewal would take the token away from
    /// a resolver that is using it.
    pub async fn set_auth_flow_renewal(
        &self,
        account_id: rd_core::AccountId,
        token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
        refresh_ref: Option<String>,
        access_ref: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetAuthFlowRenewal {
            account_id,
            token_expires_at,
            refresh_ref,
            access_ref,
            reply,
        })
        .await
    }

    /// Records the session a sign-in stored beside the account's own credential: the token
    /// and, when the sign-in left one, its key material (RD-120-30).
    ///
    /// Both references in one statement, so a token is never paired with another session's
    /// key. Creates the row when the sign-in finished before the flow service wrote one.
    pub async fn set_auth_flow_session(
        &self,
        account_id: rd_core::AccountId,
        access_ref: String,
        key_ref: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetAuthFlowSession {
            account_id,
            access_ref,
            key_ref,
            reply,
        })
        .await
    }

    /// Removes the authentication flow of one account.
    pub async fn delete_auth_flow(&self, account_id: rd_core::AccountId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteAuthFlow {
            account_id,
            reply,
        })
        .await
    }

    /// The authentication flow of one account, if it has one.
    pub async fn auth_flow(
        &self,
        account_id: rd_core::AccountId,
    ) -> Result<Option<rd_core::AuthFlow>> {
        crate::auth_flow_store::get(&self.readers, account_id).await
    }

    /// The authentication flow an arriving OAuth callback belongs to (RD-103-00).
    pub async fn auth_flow_by_callback_state(
        &self,
        callback_state: &str,
    ) -> Result<Option<rd_core::AuthFlow>> {
        crate::auth_flow_store::by_callback_state(&self.readers, callback_state).await
    }

    /// Every open authentication flow whose next poll is due.
    pub async fn due_auth_flows(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::AuthFlow>> {
        crate::auth_flow_store::due(&self.readers, now).await
    }

    /// Every authorised flow whose access token is due for renewal by `threshold` (RD-103-00).
    pub async fn due_refresh_auth_flows(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        threshold: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::AuthFlow>> {
        crate::auth_flow_store::due_refresh(&self.readers, now, threshold).await
    }

    /// Holds a renewal back after a provider asked for more time (RD-103-00).
    pub async fn defer_auth_flow_renewal(
        &self,
        account_id: rd_core::AccountId,
        next_poll_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeferAuthFlowRenewal {
            account_id,
            next_poll_at,
            reply,
        })
        .await
    }

    /// Writes the row that stands for one remote job, before the provider is asked for
    /// anything (RD-107-06).
    ///
    /// Deliberately the first write of the whole flow. The content key it carries is what
    /// makes a duplicate preventable at a provider whose submit is not idempotent: the unique
    /// index refuses a second row, so there is never a second row to drive a second submit.
    pub async fn claim_remote_job(
        &self,
        input: crate::remote_job_store::ClaimRemoteJob,
    ) -> Result<rd_core::RemoteJob> {
        writer::request(&self.writer, |reply| WriterCommand::ClaimRemoteJob {
            input: Box::new(input),
            reply,
        })
        .await
    }

    /// Records what one submit, poll or answer changed about a remote job (RD-107-06).
    pub async fn advance_remote_job(
        &self,
        id: rd_core::RemoteJobId,
        input: crate::remote_job_store::AdvanceRemoteJob,
    ) -> Result<rd_core::RemoteJob> {
        writer::request(&self.writer, |reply| WriterCommand::AdvanceRemoteJob {
            id,
            input: Box::new(input),
            reply,
        })
        .await
    }

    /// One remote job by its own identifier.
    pub async fn remote_job(&self, id: rd_core::RemoteJobId) -> Result<Option<rd_core::RemoteJob>> {
        crate::remote_job_store::get(&self.readers, id).await
    }

    /// The remote job one account already has for a content key, if any (RD-107-06).
    pub async fn remote_job_by_content(
        &self,
        account_id: rd_core::AccountId,
        content_key: &str,
    ) -> Result<Option<rd_core::RemoteJob>> {
        crate::remote_job_store::by_content(&self.readers, account_id, content_key).await
    }

    /// Every remote job of one account, newest first.
    pub async fn remote_jobs(
        &self,
        account_id: rd_core::AccountId,
    ) -> Result<Vec<rd_core::RemoteJob>> {
        crate::remote_job_store::list(&self.readers, account_id).await
    }

    /// Every remote job this installation knows about, newest first (RD-108-04).
    pub async fn all_remote_jobs(&self) -> Result<Vec<rd_core::RemoteJob>> {
        crate::remote_job_store::list_all(&self.readers).await
    }

    /// Removes one remote job's row, leaving what the provider holds untouched (RD-108-04).
    ///
    /// The other half of the pair ADR 0003 insists on keeping apart: deleting at the provider
    /// is `RemoteJobService::discard`, reached only from an explicit confirmed request, and
    /// nothing on this path calls it.
    pub async fn delete_remote_job(&self, id: rd_core::RemoteJobId) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteRemoteJob {
            id,
            reply,
        })
        .await
    }

    /// Every remote job whose next poll is due (RD-107-06).
    pub async fn due_remote_jobs(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::RemoteJob>> {
        crate::remote_job_store::due(&self.readers, now).await
    }

    /// The source a remote job was claimed with, so a restart offers the very same bytes.
    pub async fn remote_job_source(&self, id: rd_core::RemoteJobId) -> Result<Option<Vec<u8>>> {
        crate::remote_job_store::source(&self.readers, id).await
    }

    /// Lists public account metadata without secret references or values.
    pub async fn list_accounts(&self) -> Result<Vec<rd_core::Account>> {
        network_store::list_accounts(&self.readers).await
    }

    /// Lists every managed tool version this installation installed itself (RD-102-02).
    pub async fn list_managed_tools(&self) -> Result<Vec<ManagedToolRecord>> {
        managed_tools_store::list_managed_tools(&self.readers).await
    }

    /// How far the signed tool manifest has advanced, or `None` before the first refresh.
    ///
    /// The sequence is the replay floor: `rd_sign::replay::check` only refuses a replayed
    /// manifest if the caller remembers what it has already accepted.
    pub async fn tool_manifest_state(&self) -> Result<Option<ToolManifestState>> {
        managed_tools_store::tool_manifest_state(&self.readers).await
    }

    /// Records a verified, installed tool version.
    pub async fn record_managed_tool(&self, input: NewManagedTool) -> Result<ManagedToolRecord> {
        writer::request(&self.writer, |reply| WriterCommand::RecordManagedTool {
            input,
            reply,
        })
        .await
    }

    /// Forgets one installed tool version; returns whether a row was removed.
    pub async fn forget_managed_tool(&self, name: String, version: String) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::ForgetManagedTool {
            name,
            version,
            reply,
        })
        .await
    }

    /// Raises the accepted tool-manifest sequence after a manifest verified.
    pub async fn accept_tool_manifest(&self, sequence: i64, issued_at: String) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::AcceptToolManifest {
            sequence,
            issued_at,
            reply,
        })
        .await
    }

    /// Lists plugin signing keys the user confirmed on first use.
    pub async fn list_plugin_trusted_keys(&self) -> Result<Vec<PluginTrustedKey>> {
        plugin_keys_store::list_plugin_trusted_keys(&self.readers).await
    }

    /// Records a confirmed plugin signing key so installed packages still verify on restart.
    pub async fn trust_plugin_key(&self, input: NewPluginTrustedKey) -> Result<PluginTrustedKey> {
        writer::request(&self.writer, |reply| WriterCommand::TrustPluginKey {
            input,
            reply,
        })
        .await
    }

    /// Revokes a plugin signing key; returns whether one was removed.
    pub async fn revoke_plugin_key(&self, key_id: String) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::RevokePluginKey {
            key_id,
            reply,
        })
        .await
    }

    /// Every withdrawn plugin package digest, newest first.
    ///
    /// Read once at start and used to replace the verifier's in-memory set; the check itself
    /// happens in process on every load, so nothing asks this per digest.
    pub async fn list_plugin_digest_revocations(&self) -> Result<Vec<PluginDigestRevocation>> {
        plugin_revocations_store::list_plugin_digest_revocations(&self.readers).await
    }

    /// Withdraws one exact package version so it is refused the next time plugins load.
    pub async fn revoke_plugin_digest(
        &self,
        input: NewPluginDigestRevocation,
    ) -> Result<PluginDigestRevocation> {
        writer::request(&self.writer, |reply| WriterCommand::RevokePluginDigest {
            input,
            reply,
        })
        .await
    }

    /// Takes a withdrawal back; returns whether one was removed.
    pub async fn unrevoke_plugin_digest(&self, digest: String) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::UnrevokePluginDigest {
            digest,
            reply,
        })
        .await
    }

    /// Creates a proxy profile with an optional opaque credential reference.
    pub async fn create_proxy_profile(
        &self,
        input: NewProxyProfile,
    ) -> Result<rd_core::ProxyProfile> {
        writer::request(&self.writer, |reply| WriterCommand::CreateProxyProfile {
            input,
            reply,
        })
        .await
    }

    /// Replaces the editable fields of one proxy profile.
    pub async fn update_proxy_profile(
        &self,
        id: rd_core::ProxyProfileId,
        input: NewProxyProfile,
    ) -> Result<rd_core::ProxyProfile> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateProxyProfile {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes a proxy profile, returning its credential reference for the vault sweep.
    ///
    /// Refused while an account, a Usenet server or an unfinished download still points at it.
    pub async fn delete_proxy_profile(
        &self,
        id: rd_core::ProxyProfileId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteProxyProfile {
            id,
            reply,
        })
        .await
    }

    /// Lists proxy profiles. Serialization omits their opaque secret reference.
    pub async fn list_proxy_profiles(&self) -> Result<Vec<rd_core::ProxyProfile>> {
        network_store::list_proxy_profiles(&self.readers).await
    }

    /// Loads one proxy profile, for callers that need nothing else from the network config.
    pub async fn proxy_profile(
        &self,
        id: rd_core::ProxyProfileId,
    ) -> Result<Option<rd_core::ProxyProfile>> {
        network_store::load_proxy(&self.readers, id).await
    }

    /// Resolves job, account, proxy and auth-profile metadata using the required precedence.
    pub async fn network_client_config(
        &self,
        account_id: Option<rd_core::AccountId>,
        job_proxy_id: Option<rd_core::ProxyProfileId>,
        global_proxy_id: Option<rd_core::ProxyProfileId>,
        auth_profile: rd_core::AuthProfileSelection,
        url: &url::Url,
    ) -> Result<NetworkClientConfig> {
        network_store::client_config(
            &self.readers,
            account_id,
            job_proxy_id,
            global_proxy_id,
            auth_profile,
            url,
        )
        .await
    }

    /// Persists one redaction-safe NNTP server configuration.
    pub async fn create_usenet_server(
        &self,
        input: NewUsenetServer,
    ) -> Result<rd_core::UsenetServer> {
        writer::request(&self.writer, |reply| WriterCommand::CreateUsenetServer {
            input,
            reply,
        })
        .await
    }

    /// Updates one redaction-safe NNTP server configuration.
    pub async fn update_usenet_server(
        &self,
        id: rd_core::UsenetServerId,
        input: UpdateUsenetServer,
    ) -> Result<rd_core::UsenetServer> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateUsenetServer {
            id,
            input,
            reply,
        })
        .await
    }

    /// Deletes one NNTP endpoint and returns its opaque password reference for cleanup.
    pub async fn delete_usenet_server(
        &self,
        id: rd_core::UsenetServerId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteUsenetServer {
            id,
            reply,
        })
        .await
    }

    /// Lists NNTP endpoints in fallback priority order.
    pub async fn list_usenet_servers(&self) -> Result<Vec<rd_core::UsenetServer>> {
        usenet_store::list(&self.readers).await
    }

    /// Loads one enabled NNTP endpoint with its opaque password reference.
    pub async fn usenet_connection_config(
        &self,
        id: rd_core::UsenetServerId,
    ) -> Result<Option<UsenetConnectionConfig>> {
        usenet_store::connection_config(&self.readers, id).await
    }

    /// Persists a JSON setting.
    pub async fn set_setting(&self, key: String, value: serde_json::Value) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetSetting {
            key,
            value,
            reply,
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

/// Converts a stored textual identifier into a domain identifier.
pub(crate) fn parse_id<T>(value: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.parse::<T>().context("parse stored identifier")
}
