//! The serialized writer: the single task every mutation in the process goes through.
//!
//! [`Writer::run`] owns the one write connection and applies commands strictly in the order
//! they were sent. It routes each [`WriterCommand`] to the module that owns the store behind
//! it — the split follows `src/*_store.rs`, so the writer half of `notify_store` is in
//! [`notify`] — and the helpers below are what those modules share.

use anyhow::{Context, Result};
use rd_core::EventEnvelope;
use sqlx::SqliteConnection;
use tokio::sync::{mpsc, oneshot};

use crate::commands::{Reply, WriterCommand};

mod audit;
mod auth;
mod bandwidth;
mod collector;
mod config;
mod download_rows;
mod downloads;
mod logs;
mod maintenance;
mod network;
mod notify;
mod nzb;
mod packages;
mod plugins;
mod sessions;
mod streams;
mod subscriptions;

pub(crate) struct Writer {
    pub(crate) connection: SqliteConnection,
    commands: mpsc::Receiver<WriterCommand>,
    pub(crate) events: crate::EventBus,
    /// Last broadcast progress event per download (events are throttled, not persisted).
    last_progress: std::collections::HashMap<String, std::time::Instant>,
}

const PROGRESS_EVENT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);

impl Writer {
    pub(crate) fn new(
        connection: SqliteConnection,
        commands: mpsc::Receiver<WriterCommand>,
        events: crate::EventBus,
    ) -> Self {
        Self {
            last_progress: std::collections::HashMap::new(),
            connection,
            commands,
            events,
        }
    }

    pub(crate) async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            // Exhaustive over `WriterCommand`: a new variant does not compile until it is
            // routed, which is what keeps a command from being accepted and silently dropped.
            match command {
                command @ (WriterCommand::CreatePackage { .. }
                | WriterCommand::CreateDownload { .. }
                | WriterCommand::TransitionDownload { .. }
                | WriterCommand::BlockDownload { .. }
                | WriterCommand::DeleteDownload { .. }
                | WriterCommand::DeleteEmptyPackage { .. }
                | WriterCommand::CheckpointChunk { .. }
                | WriterCommand::CheckpointChunkMac { .. }
                | WriterCommand::SetDownloadProgress { .. }
                | WriterCommand::SetCandidateTorrentState { .. }
                | WriterCommand::SetDownloadTorrentState { .. }
                | WriterCommand::PrepareTransfer { .. }
                | WriterCommand::RecordFailure { .. }
                | WriterCommand::CompleteDownload { .. }
                | WriterCommand::RenameDownload { .. }
                | WriterCommand::SetFileName { .. }
                | WriterCommand::SetTransformKeyRef { .. }
                | WriterCommand::ClaimResolverRefresh { .. }
                | WriterCommand::ClaimReplayRefresh { .. }
                | WriterCommand::ResetDownload { .. }
                | WriterCommand::ResetTransfer { .. }
                | WriterCommand::SetCandidateReplayConsent { .. }
                | WriterCommand::ClaimResolverPin { .. }
                | WriterCommand::ClearUnsatisfiableResolverPins { .. }) => {
                    self.handle_downloads(command).await
                }
                command @ (WriterCommand::CarryEnrichment { .. }
                | WriterCommand::UpdatePackages { .. }
                | WriterCommand::RenamePackageDirectory { .. }
                | WriterCommand::ClearPreviousDestination { .. }
                | WriterCommand::ReorderPackages { .. }
                | WriterCommand::ReorderDownloads { .. }
                | WriterCommand::SetPackageState { .. }
                | WriterCommand::SetPackageExtraction { .. }) => {
                    self.handle_packages(command).await
                }
                command @ (WriterCommand::AddMediaCandidates { .. }
                | WriterCommand::SetCandidateEnrichment { .. }
                | WriterCommand::SetCandidateMediaInventory { .. }
                | WriterCommand::SetCandidateMediaSelection { .. }
                | WriterCommand::SetCandidateMediaVariant { .. }
                | WriterCommand::SetCandidateProvider { .. }
                | WriterCommand::SetCandidateAuthProfile { .. }
                | WriterCommand::AddCollectorBatch { .. }
                | WriterCommand::UpdateCollectorPackages { .. }
                | WriterCommand::ReorderCollectorPackages { .. }
                | WriterCommand::ReorderGrabberEntries { .. }
                | WriterCommand::ReorderCandidates { .. }
                | WriterCommand::MoveCandidates { .. }
                | WriterCommand::DeleteCollectorPackage { .. }
                | WriterCommand::RegroupBatches { .. }
                | WriterCommand::SetMirrorPreference { .. }
                | WriterCommand::SetMirrorPin { .. }
                | WriterCommand::DissolveMirrorGroup { .. }
                | WriterCommand::ClaimCandidatesForCheck { .. }
                | WriterCommand::RecordCandidateCheck { .. }
                | WriterCommand::MarkCandidateUnsupported { .. }
                | WriterCommand::SetCandidateFileName { .. }
                | WriterCommand::ClaimPackageForEnqueue { .. }
                | WriterCommand::FinishPackageEnqueue { .. }
                | WriterCommand::DeleteCandidate { .. }
                | WriterCommand::DeleteCandidates { .. }) => self.handle_collector(command).await,
                command @ (WriterCommand::SettleNzbRecovery { .. }
                | WriterCommand::DeferPar2Verdict { .. }
                | WriterCommand::AddNzbImport { .. }
                | WriterCommand::RecordNzbImportFailure { .. }
                | WriterCommand::UpdateNzbImport { .. }
                | WriterCommand::DeleteNzbImport { .. }
                | WriterCommand::ForgetNzbImportHistory { .. }
                | WriterCommand::SetNzbSegmentState { .. }
                | WriterCommand::EnqueueNzbImport { .. }
                | WriterCommand::CheckpointNzb { .. }) => self.handle_nzb(command).await,
                command @ (WriterCommand::SetCategorySeedingPolicy { .. }
                | WriterCommand::CreateStorageRoot { .. }
                | WriterCommand::UpdateStorageRoot { .. }
                | WriterCommand::DeleteStorageRoot { .. }
                | WriterCommand::CreateCategory { .. }
                | WriterCommand::UpdateCategory { .. }
                | WriterCommand::DeleteCategory { .. }
                | WriterCommand::UpdateCategoryPostprocess { .. }
                | WriterCommand::CreateCategoryRule { .. }
                | WriterCommand::UpdateCategoryRule { .. }
                | WriterCommand::DeleteCategoryRule { .. }
                | WriterCommand::CreateHotFolder { .. }
                | WriterCommand::UpdateHotFolder { .. }
                | WriterCommand::DeleteHotFolder { .. }
                | WriterCommand::UpsertSiteRule { .. }
                | WriterCommand::DeleteSiteRule { .. }
                | WriterCommand::SetSiteRuleSwitch { .. }
                | WriterCommand::RecordSiteRuleChecks { .. }) => self.handle_config(command).await,
                command @ (WriterCommand::CreateAccount { .. }
                | WriterCommand::UpdateAccount { .. }
                | WriterCommand::DeleteAccount { .. }
                | WriterCommand::CreateProxyProfile { .. }
                | WriterCommand::UpdateProxyProfile { .. }
                | WriterCommand::DeleteProxyProfile { .. }
                | WriterCommand::CreateUsenetServer { .. }
                | WriterCommand::UpdateUsenetServer { .. }
                | WriterCommand::DeleteUsenetServer { .. }
                | WriterCommand::CreateRemoteCredential { .. }
                | WriterCommand::UpdateRemoteCredential { .. }
                | WriterCommand::DeleteRemoteCredential { .. }
                | WriterCommand::TrustSshHostKey { .. }
                | WriterCommand::ForgetSshHostKey { .. }
                | WriterCommand::SetCandidateListing { .. }
                | WriterCommand::SetCandidateListingPlan { .. }) => {
                    self.handle_network(command).await
                }
                command @ (WriterCommand::UpsertAuthFlow { .. }
                | WriterCommand::SetAuthFlowRenewal { .. }
                | WriterCommand::SetAuthFlowSession { .. }
                | WriterCommand::DeferAuthFlowRenewal { .. }
                | WriterCommand::DeleteAuthFlow { .. }
                | WriterCommand::SetDownloadAuthProfile { .. }
                | WriterCommand::CreateAuthProfile { .. }
                | WriterCommand::UpdateAuthProfile { .. }
                | WriterCommand::SetAuthProfileEnabled { .. }
                | WriterCommand::DeleteAuthProfile { .. }) => self.handle_auth(command).await,
                command @ (WriterCommand::CreateSession { .. }
                | WriterCommand::TouchSession { .. }
                | WriterCommand::RevokeSession { .. }
                | WriterCommand::RevokeOtherSessions { .. }
                | WriterCommand::RevokeAllSessions { .. }
                | WriterCommand::PurgeExpiredSessions { .. }
                | WriterCommand::TouchCaptureToken { .. }
                | WriterCommand::CreateCaptureToken { .. }
                | WriterCommand::UpdateCaptureTokenScopes { .. }
                | WriterCommand::RevokeCaptureToken { .. }
                | WriterCommand::CreateMfaCredential { .. }
                | WriterCommand::ConfirmMfaCredential { .. }
                | WriterCommand::RepointMfaMaterial { .. }
                | WriterCommand::TouchMfaCredential { .. }
                | WriterCommand::AcceptTotpStep { .. }
                | WriterCommand::DeleteMfaCredential { .. }
                | WriterCommand::ReplaceRecoveryCodes { .. }
                | WriterCommand::SpendRecoveryCode { .. }
                | WriterCommand::ClearMfa { .. }) => self.handle_sessions(command).await,
                command @ (WriterCommand::SavePluginTransfer { .. }
                | WriterCommand::ClearPluginTransfer { .. }
                | WriterCommand::RecordPluginExecution { .. }
                | WriterCommand::TrustPluginKey { .. }
                | WriterCommand::RevokePluginKey { .. }
                | WriterCommand::RevokePluginDigest { .. }
                | WriterCommand::UnrevokePluginDigest { .. }
                | WriterCommand::ClaimRemoteJob { .. }
                | WriterCommand::AdvanceRemoteJob { .. }
                | WriterCommand::DeleteRemoteJob { .. }
                | WriterCommand::RecordManagedTool { .. }
                | WriterCommand::ForgetManagedTool { .. }
                | WriterCommand::AcceptToolManifest { .. }) => self.handle_plugins(command).await,
                command @ (WriterCommand::SetDownloadRecordingState { .. }
                | WriterCommand::CreateStreamSchedule { .. }
                | WriterCommand::UpdateStreamSchedule { .. }
                | WriterCommand::DeleteStreamSchedule { .. }
                | WriterCommand::PlanStreamRuns { .. }
                | WriterCommand::SetStreamRunState { .. }
                | WriterCommand::ExpireStreamRuns { .. }
                | WriterCommand::CreateStreamChannel { .. }
                | WriterCommand::UpdateStreamChannel { .. }
                | WriterCommand::DeleteStreamChannel { .. }
                | WriterCommand::TouchStreamChannel { .. }) => self.handle_streams(command).await,
                command @ (WriterCommand::CreateSubscription { .. }
                | WriterCommand::UpdateSubscription { .. }
                | WriterCommand::SetSubscriptionEnabled { .. }
                | WriterCommand::DeleteSubscription { .. }
                | WriterCommand::RecordSubscriptionItems { .. }
                | WriterCommand::SetSubscriptionItemState { .. }
                | WriterCommand::SetPendingSubscriptionItemsState { .. }
                | WriterCommand::ClearSubscriptionHistory { .. }
                | WriterCommand::ArmSubscription { .. }
                | WriterCommand::FinishSubscriptionRun { .. }) => {
                    self.handle_subscriptions(command).await
                }
                command @ (WriterCommand::UpsertNotificationTarget { .. }
                | WriterCommand::DeleteNotificationTarget { .. }
                | WriterCommand::UpsertNotificationRule { .. }
                | WriterCommand::DeleteNotificationRule { .. }
                | WriterCommand::QueueNotificationDelivery { .. }
                | WriterCommand::RecordNotificationAttempt { .. }
                | WriterCommand::ClearNotificationDeliveries { .. }
                | WriterCommand::UpsertAutomation { .. }
                | WriterCommand::SetAutomationEnabled { .. }
                | WriterCommand::DeleteAutomation { .. }
                | WriterCommand::QueueAutomationRun { .. }
                | WriterCommand::RecordAutomationAttempt { .. }
                | WriterCommand::RecoverAutomationRuns { .. }) => self.handle_notify(command).await,
                command @ (WriterCommand::CreateBandwidthProfile { .. }
                | WriterCommand::UpdateBandwidthProfile { .. }
                | WriterCommand::DeleteBandwidthProfile { .. }
                | WriterCommand::ReplaceBandwidthWindows { .. }
                | WriterCommand::StoreBandwidthBudget { .. }) => {
                    self.handle_bandwidth(command).await
                }
                command @ (WriterCommand::ReplaceConfig { .. }
                | WriterCommand::SetSetting { .. }
                | WriterCommand::CheckpointWal { .. }
                | WriterCommand::RecoverInterrupted { .. }
                | WriterCommand::PurgeOldEvents { .. }
                | WriterCommand::PruneTransferStats { .. }
                | WriterCommand::ClearTransferStats { .. }) => {
                    self.handle_maintenance(command).await
                }
                command @ (WriterCommand::AppendLogRecords { .. }
                | WriterCommand::PruneLogRecords { .. }
                | WriterCommand::ClearLogRecords { .. }) => self.handle_logs(command).await,
                command @ (WriterCommand::AppendAuditRecords { .. }
                | WriterCommand::PruneAuditRecords { .. }
                | WriterCommand::ClearAuditRecords { .. }) => self.handle_audit(command).await,
            }
        }
    }
}

/// How long a persisted event is kept.
///
/// The `events` table is append-only and nothing in the workspace reads it — the live view
/// goes through the broadcast channel, not through here. It is kept because it is the only
/// record of what happened before the process started, and an audit view or a history page
/// is a plausible thing to want; without a sweep it was simply the largest table in the file
/// on any busy install, growing for the life of the service.
const EVENT_RETENTION_DAYS: i64 = 30;

async fn purge_old_events(connection: &mut sqlx::SqliteConnection) -> Result<u64> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(EVENT_RETENTION_DAYS);
    let result = sqlx::query("DELETE FROM events WHERE occurred_at < ?")
        .bind(cutoff)
        .execute(connection)
        .await
        .context("purge old events")?;
    Ok(result.rows_affected())
}

pub(crate) async fn request<T>(
    writer: &mpsc::Sender<WriterCommand>,
    make: impl FnOnce(Reply<T>) -> WriterCommand,
) -> Result<T> {
    let (reply_tx, reply_rx) = oneshot::channel();
    writer
        .send(make(reply_tx))
        .await
        .context("database writer stopped")?;
    reply_rx.await.context("database writer dropped response")?
}

fn send<T>(reply: Reply<T>, result: Result<T>) {
    let _ = reply.send(result);
}

fn publish_config<T>(
    reply: Reply<T>,
    result: Result<(T, EventEnvelope)>,
    events: &crate::EventBus,
) {
    if let Ok((_, event)) = &result {
        let _ = events.send(event.clone());
    }
    send(reply, result.map(|(value, _)| value));
}

fn publish_unit_event(reply: Reply<()>, result: Result<EventEnvelope>, events: &crate::EventBus) {
    if let Ok(event) = &result {
        let _ = events.send(event.clone());
    }
    send(reply, result.map(|_| ()));
}

pub(crate) async fn insert_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event: &EventEnvelope,
) -> Result<()> {
    sqlx::query("INSERT INTO events (id, kind, occurred_at, payload_json) VALUES (?, ?, ?, ?)")
        .bind(event.id.to_string())
        .bind(
            serde_json::to_string(&event.kind)?
                .trim_matches('"')
                .to_owned(),
        )
        .bind(event.occurred_at)
        .bind(serde_json::to_string(&event.payload)?)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Serialised `DownloadKind` for TEXT columns.
pub(crate) fn kind_string(kind: rd_core::DownloadKind) -> String {
    serde_json::to_string(&kind)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}

/// Serialised `PostprocessLevel` for TEXT columns.
pub(crate) fn level_string(level: rd_core::PostprocessLevel) -> String {
    serde_json::to_string(&level)
        .unwrap_or_default()
        .trim_matches('"')
        .to_owned()
}
