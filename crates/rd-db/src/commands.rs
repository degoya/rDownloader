use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{
    Category, CategoryRule, ChunkId, DownloadFile, DownloadId, DownloadState, HotFolderConfig,
    StorageRootConfig,
};
use tokio::sync::oneshot;

use crate::{
    config_store::{NewCategory, NewCategoryRule, NewHotFolder, NewStorageRoot},
    models::{NewDownload, NewPackage, PersistedChunk},
    network_store::{NewAccount, NewProxyProfile, UpdateAccount},
    nzb_store::NewNzbImport,
    usenet_store::{NewUsenetServer, UpdateUsenetServer},
};

pub(crate) type Reply<T> = oneshot::Sender<Result<T>>;

pub(crate) enum WriterCommand {
    ReplaceConfig {
        replacement: crate::backup_store::ConfigReplacement,
        reply: Reply<()>,
    },
    CreatePackage {
        package: NewPackage,
        reply: Reply<rd_core::DownloadPackage>,
    },
    CreateDownload {
        download: NewDownload,
        reply: Reply<DownloadFile>,
    },
    TransitionDownload {
        id: DownloadId,
        next: DownloadState,
        reply: Reply<DownloadFile>,
    },
    /// Blocks a download and records why, so a release path can tell the causes apart.
    BlockDownload {
        id: DownloadId,
        /// Stable identifier of the cause; the caller owns the vocabulary.
        reason: String,
        reply: Reply<DownloadFile>,
    },
    DeleteDownload {
        id: DownloadId,
        reply: Reply<()>,
    },
    /// Removes a package that has no files, for a caller with no download id to offer.
    DeleteEmptyPackage {
        id: rd_core::PackageId,
        reply: Reply<bool>,
    },
    CheckpointChunk {
        chunk_id: ChunkId,
        committed_offset: u64,
        reply: Reply<()>,
    },
    /// Records one finished provider-chunk MAC of a transformed stream (RD-103-02).
    CheckpointChunkMac {
        download_id: DownloadId,
        /// `ContentTransform::fingerprint` of the description that produced it. A row
        /// written by any other description is dropped rather than mixed in.
        fingerprint: String,
        index: u64,
        mac: [u8; 16],
        reply: Reply<()>,
    },
    /// Progress of a runner-driven file (no chunk rows), e.g. media downloads.
    SetDownloadProgress {
        id: DownloadId,
        committed_bytes: u64,
        total_bytes: Option<u64>,
        reply: Reply<()>,
    },
    /// Replaces the torrent state blob of one link candidate.
    SetCandidateTorrentState {
        id: rd_core::CandidateId,
        /// Boxed: the file tree makes this by far the largest command variant.
        state: Box<rd_core::TorrentCandidateState>,
        reply: Reply<()>,
    },
    /// Replaces the seeding override of one category.
    SetCategorySeedingPolicy {
        id: rd_core::CategoryId,
        policy: Option<rd_core::SeedingPolicyOverride>,
        reply: Reply<()>,
    },
    /// Replaces the torrent state blob of one queue row.
    SetDownloadTorrentState {
        id: DownloadId,
        state: Box<rd_core::TorrentJobState>,
        reply: Reply<()>,
    },
    /// Appends probed playlist entries to an existing LinkGrabber package.
    AddMediaCandidates {
        package_id: rd_core::CollectorPackageId,
        entries: Vec<rd_core::MediaCandidate>,
        reply: Reply<Vec<rd_core::LinkCandidate>>,
    },
    SetCandidateMediaVariant {
        id: rd_core::CandidateId,
        variant_id: String,
        reply: Reply<rd_core::LinkCandidate>,
    },
    /// Stores the probed format inventory of a media candidate (RD-080-01).
    /// Creates or advances an authentication flow (RD-090-13).
    UpsertAuthFlow {
        input: Box<crate::auth_flow_store::UpsertAuthFlow>,
        reply: Reply<rd_core::AuthFlow>,
    },
    /// Records the expiry and refresh reference a renewal produced (RD-103-00).
    SetAuthFlowRenewal {
        account_id: rd_core::AccountId,
        token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
        refresh_ref: Option<String>,
        /// `None` leaves whatever is stored alone; the token only moves when a caller says so.
        access_ref: Option<String>,
        reply: Reply<()>,
    },
    /// Records a sign-in's session beside the account's own credential (RD-120-30).
    SetAuthFlowSession {
        account_id: rd_core::AccountId,
        access_ref: String,
        key_ref: Option<String>,
        reply: Reply<()>,
    },
    /// Holds a renewal back after a provider asked for more time (RD-103-00).
    DeferAuthFlowRenewal {
        account_id: rd_core::AccountId,
        next_poll_at: chrono::DateTime<chrono::Utc>,
        reply: Reply<()>,
    },
    /// Removes an authentication flow.
    DeleteAuthFlow {
        account_id: rd_core::AccountId,
        reply: Reply<()>,
    },
    /// Writes the row that stands for one remote job, before the provider is asked for
    /// anything (RD-107-06). Refused when the account already has one for that content.
    ClaimRemoteJob {
        input: Box<crate::remote_job_store::ClaimRemoteJob>,
        reply: Reply<rd_core::RemoteJob>,
    },
    /// Records what one submit, poll or answer changed about a remote job (RD-107-06).
    AdvanceRemoteJob {
        id: rd_core::RemoteJobId,
        input: Box<crate::remote_job_store::AdvanceRemoteJob>,
        reply: Reply<rd_core::RemoteJob>,
    },
    /// Removes one remote job's row and nothing at the provider (RD-108-04).
    DeleteRemoteJob {
        id: rd_core::RemoteJobId,
        reply: Reply<bool>,
    },
    /// Stores the fields an enricher plugin contributed to a candidate (RD-090-14).
    SetCandidateEnrichment {
        id: rd_core::CandidateId,
        fields: Vec<rd_core::EnrichmentField>,
        reply: Reply<()>,
    },
    /// Carries those fields onto the package and files an enqueue created (RD-107-02).
    CarryEnrichment {
        package_id: rd_core::PackageId,
        /// The union across the package's files, shown on the package header.
        package_fields: Vec<rd_core::EnrichmentField>,
        /// Per queue row, so a package of several releases keeps them apart.
        files: Vec<(rd_core::DownloadId, Vec<rd_core::EnrichmentField>)>,
        reply: Reply<()>,
    },
    SetCandidateMediaInventory {
        id: rd_core::CandidateId,
        state: Box<rd_core::MediaCandidateState>,
        reply: Reply<()>,
    },
    /// Stores a resolved format selection (RD-080-01). Boxed because the update carries a
    /// whole variant and its criteria, which would otherwise widen every command.
    SetCandidateMediaSelection {
        id: rd_core::CandidateId,
        update: Box<rd_core::MediaSelectionUpdate>,
        reply: Reply<rd_core::LinkCandidate>,
    },
    /// Segment history and sidecars of a recording (RD-080-09).
    SetDownloadRecordingState {
        id: rd_core::DownloadId,
        state: Box<rd_core::RecordingState>,
        reply: Reply<()>,
    },
    /// Livestream schedules (RD-080-08).
    CreateStreamSchedule {
        input: Box<crate::stream_schedule_store::NewStreamSchedule>,
        reply: Reply<rd_core::StreamSchedule>,
    },
    UpdateStreamSchedule {
        id: rd_core::StreamScheduleId,
        input: Box<crate::stream_schedule_store::NewStreamSchedule>,
        reply: Reply<rd_core::StreamSchedule>,
    },
    DeleteStreamSchedule {
        id: rd_core::StreamScheduleId,
        reply: Reply<()>,
    },
    /// Inserts occurrences that are not planned yet; returns how many were new.
    PlanStreamRuns {
        schedule_id: rd_core::StreamScheduleId,
        channel_id: rd_core::StreamChannelId,
        occurrences: Vec<crate::stream_schedule_store::PlannedOccurrence>,
        reply: Reply<u32>,
    },
    SetStreamRunState {
        id: rd_core::StreamScheduledRunId,
        state: rd_core::ScheduledRunState,
        download_id: Option<rd_core::DownloadId>,
        replay_used: Option<bool>,
        error: Option<String>,
        reply: Reply<()>,
    },
    /// Marks open runs whose window has closed as missed.
    ExpireStreamRuns {
        cutoff: chrono::DateTime<chrono::Utc>,
        reply: Reply<u32>,
    },
    /// Subscriptions (RD-080-07).
    CreateSubscription {
        input: Box<crate::subscription_store::NewSubscription>,
        reply: Reply<rd_core::Subscription>,
    },
    UpdateSubscription {
        id: rd_core::SubscriptionId,
        input: Box<crate::subscription_store::NewSubscription>,
        /// The subscription, plus the secret reference the edit replaced, if any.
        reply: Reply<(rd_core::Subscription, Option<String>)>,
    },
    SetSubscriptionEnabled {
        id: rd_core::SubscriptionId,
        enabled: bool,
        reply: Reply<rd_core::Subscription>,
    },
    DeleteSubscription {
        id: rd_core::SubscriptionId,
        /// The secret reference to drop from the vault, if the subscription had one.
        reply: Reply<Option<String>>,
    },
    RecordSubscriptionItems {
        subscription_id: rd_core::SubscriptionId,
        items: Vec<crate::subscription_store::NewSubscriptionItem>,
        /// Only the rows this call created; the rest were already archived.
        reply: Reply<Vec<rd_core::SubscriptionItem>>,
    },
    SetSubscriptionItemState {
        id: rd_core::SubscriptionItemId,
        state: rd_core::SubscriptionItemState,
        reply: Reply<()>,
    },
    SetPendingSubscriptionItemsState {
        ids: Vec<rd_core::SubscriptionItemId>,
        state: rd_core::SubscriptionItemState,
        reply: Reply<u64>,
    },
    ClearSubscriptionHistory {
        id: rd_core::SubscriptionId,
        reply: Reply<rd_core::SubscriptionHistoryClearResponse>,
    },
    /// Times a scheduled subscription's first run (RD-130-19); `false` when it already had one.
    ArmSubscription {
        id: rd_core::SubscriptionId,
        next_run_at: chrono::DateTime<chrono::Utc>,
        reply: Reply<bool>,
    },
    FinishSubscriptionRun {
        subscription_id: rd_core::SubscriptionId,
        started_at: chrono::DateTime<chrono::Utc>,
        result: Box<crate::subscription_store::PollResult>,
        reply: Reply<()>,
    },
    /// Re-routes a candidate to another provider after the check learned what it is
    /// (RD-080-06).
    SetCandidateProvider {
        id: rd_core::CandidateId,
        provider: String,
        reply: Reply<rd_core::LinkCandidate>,
    },
    /// Sets the cookie/authentication profile a candidate is queued with (RD-080-04).
    SetCandidateAuthProfile {
        id: rd_core::CandidateId,
        selection: rd_core::AuthProfileSelection,
        reply: Reply<rd_core::LinkCandidate>,
    },
    PrepareTransfer {
        id: DownloadId,
        total_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        chunks: Vec<PersistedChunk>,
        reply: Reply<()>,
    },
    RecordFailure {
        id: DownloadId,
        failure: rd_core::Failure,
        retry_at: Option<DateTime<Utc>>,
        reply: Reply<DownloadFile>,
    },
    CompleteDownload {
        id: DownloadId,
        final_name: String,
        checksum: Option<rd_core::ExpectedChecksum>,
        reply: Reply<DownloadFile>,
    },
    SetFileName {
        id: DownloadId,
        file_name: String,
        reply: Reply<()>,
    },
    /// The vault reference of this download's transform key (RD-120-11).
    ///
    /// `None` clears it, which is what a download whose transform went away needs; the vault
    /// entry itself is removed by the caller, because the writer owns rows and not files.
    SetTransformKeyRef {
        id: DownloadId,
        reference: Option<String>,
        reply: Reply<()>,
    },
    RenameDownload {
        id: DownloadId,
        file_name: String,
        reply: Reply<DownloadFile>,
    },
    /// The assembled file of a Usenet row is on disk: final name, PAR2 marking decided again
    /// on that name and the content, and the set's waiting volumes postponed if this is the
    /// main index (RD-108-23). Replies with the number of volumes postponed.
    SettleNzbRecovery {
        id: DownloadId,
        file_name: String,
        /// The file starts with the PAR2 packet magic.
        content_is_par2: bool,
        reply: Reply<usize>,
    },
    /// A Usenet file is assembled but has holes where articles were missing, and whether the
    /// set can repair them is not yet known (RD-108-24). Holds the verdict open on the row
    /// until the package settles.
    DeferPar2Verdict {
        id: DownloadId,
        /// Segments no server had.
        missing: usize,
        reply: Reply<()>,
    },
    ClaimResolverRefresh {
        id: DownloadId,
        reply: Reply<bool>,
    },
    ClaimReplayRefresh {
        id: DownloadId,
        reply: Reply<bool>,
    },
    ResetDownload {
        id: DownloadId,
        reply: Reply<rd_core::DownloadFile>,
    },
    ResetTransfer {
        id: DownloadId,
        reply: Reply<()>,
    },
    SetCandidateReplayConsent {
        id: rd_core::CandidateId,
        consent: Box<Option<rd_core::ReplayConsent>>,
        reply: Reply<()>,
    },
    ClaimResolverPin {
        id: DownloadId,
        pin: rd_core::ResolverPin,
        reply: Reply<rd_core::ResolverPin>,
    },
    SavePluginTransfer {
        id: DownloadId,
        plugin_id: String,
        plugin_version: String,
        checkpoint: Option<Vec<u8>>,
        reply: Reply<crate::PluginTransfer>,
    },
    ClearPluginTransfer {
        id: DownloadId,
        reply: Reply<()>,
    },
    RecordPluginExecution {
        entry: Box<crate::NewPluginExecution>,
        reply: Reply<()>,
    },
    ClearUnsatisfiableResolverPins {
        /// `(plugin_id, version)` of every resolver this build can actually provide.
        available: Vec<(String, String)>,
        reply: Reply<u64>,
    },
    RecoverInterrupted {
        reply: Reply<u64>,
    },
    CheckpointWal {
        reply: Reply<()>,
    },
    SetSetting {
        key: String,
        value: serde_json::Value,
        reply: Reply<()>,
    },
    TrustPluginKey {
        input: crate::NewPluginTrustedKey,
        reply: Reply<crate::PluginTrustedKey>,
    },
    /// Withdraws one exact plugin package by its content digest.
    RevokePluginDigest {
        input: crate::NewPluginDigestRevocation,
        reply: Reply<crate::PluginDigestRevocation>,
    },
    /// Takes such a withdrawal back.
    UnrevokePluginDigest {
        digest: String,
        reply: Reply<bool>,
    },
    RecordManagedTool {
        input: crate::NewManagedTool,
        reply: Reply<crate::ManagedToolRecord>,
    },
    ForgetManagedTool {
        name: String,
        version: String,
        reply: Reply<bool>,
    },
    AcceptToolManifest {
        sequence: i64,
        issued_at: String,
        reply: Reply<()>,
    },
    RevokePluginKey {
        key_id: String,
        reply: Reply<bool>,
    },
    CreateAccount {
        input: NewAccount,
        reply: Reply<rd_core::Account>,
    },
    UpdateAccount {
        id: rd_core::AccountId,
        input: UpdateAccount,
        reply: Reply<rd_core::Account>,
    },
    DeleteAccount {
        id: rd_core::AccountId,
        reply: Reply<(Option<String>, Option<String>)>,
    },
    CreateProxyProfile {
        input: NewProxyProfile,
        reply: Reply<rd_core::ProxyProfile>,
    },
    UpdateProxyProfile {
        id: rd_core::ProxyProfileId,
        input: NewProxyProfile,
        reply: Reply<rd_core::ProxyProfile>,
    },
    DeleteProxyProfile {
        id: rd_core::ProxyProfileId,
        reply: Reply<Option<String>>,
    },
    CreateUsenetServer {
        input: NewUsenetServer,
        reply: Reply<rd_core::UsenetServer>,
    },
    UpdateUsenetServer {
        id: rd_core::UsenetServerId,
        input: UpdateUsenetServer,
        reply: Reply<rd_core::UsenetServer>,
    },
    DeleteUsenetServer {
        id: rd_core::UsenetServerId,
        reply: Reply<Option<String>>,
    },
    AddCollectorBatch {
        intake: crate::collector_store::NewCollectorBatch,
        /// `vault://` reference per link, parallel to `intake.urls` (RD-110-38). Filled by
        /// `Database::add_collector_batch`, which is the only place that holds the vault.
        secret_fragment_refs: Vec<Option<String>>,
        reply: Reply<(
            rd_core::CollectorBatch,
            Vec<rd_core::CollectorPackage>,
            Vec<rd_core::LinkCandidate>,
        )>,
    },
    UpdateCollectorPackages {
        ids: Vec<rd_core::CollectorPackageId>,
        change: crate::collector_packages::CollectorPackageChange,
        reply: Reply<Vec<rd_core::CollectorPackage>>,
    },
    ReorderCollectorPackages {
        ids: Vec<rd_core::CollectorPackageId>,
        reply: Reply<()>,
    },
    ReorderGrabberEntries {
        entries: Vec<rd_core::GrabberEntryRef>,
        after: Option<rd_core::GrabberEntryRef>,
        reply: Reply<()>,
    },
    ReorderCandidates {
        package_id: rd_core::CollectorPackageId,
        ids: Vec<rd_core::CandidateId>,
        reply: Reply<()>,
    },
    MoveCandidates {
        ids: Vec<rd_core::CandidateId>,
        target: crate::collector_packages::MoveTarget,
        reply: Reply<rd_core::CollectorPackage>,
    },
    DeleteCollectorPackage {
        id: rd_core::CollectorPackageId,
        reply: Reply<()>,
    },
    RegroupBatches {
        batch_ids: Vec<rd_core::BatchId>,
        reply: Reply<()>,
    },
    /// Stores the standing mirror preference and re-chooses every group under it (RD-110-19).
    SetMirrorPreference {
        preference: rd_core::MirrorPreference,
        reply: Reply<()>,
    },
    /// Pins one candidate as its group's chosen mirror, or releases that pin.
    ///
    /// `false` means the link is in no mirror group, which the REST layer refuses.
    SetMirrorPin {
        id: rd_core::CandidateId,
        pinned: bool,
        reply: Reply<bool>,
    },
    /// Takes a proposed mirror group apart and records that its links differ (RD-110-34).
    DissolveMirrorGroup {
        id: rd_core::CandidateId,
        reply: Reply<crate::MirrorDissolve>,
    },
    ClaimCandidatesForCheck {
        ids: Vec<rd_core::CandidateId>,
        reply: Reply<Vec<rd_core::LinkCandidate>>,
    },
    RecordCandidateCheck {
        id: rd_core::CandidateId,
        result: Option<rd_core::LinkCheckResult>,
        error: Option<rd_core::CandidateMessage>,
        /// Restores the duplicate state after the check so the warning survives.
        was_duplicate: bool,
        /// The provider whose cache answered; kept only with a `cached` result (RD-130-11).
        cached_by: Option<String>,
        reply: Reply<()>,
    },
    MarkCandidateUnsupported {
        id: rd_core::CandidateId,
        message: rd_core::CandidateMessage,
        /// The provider whose cache holds the file although no check source exists.
        cached_by: Option<String>,
        reply: Reply<()>,
    },
    SetCandidateFileName {
        id: rd_core::CandidateId,
        file_name: String,
        reply: Reply<rd_core::LinkCandidate>,
    },
    ClaimPackageForEnqueue {
        id: rd_core::CollectorPackageId,
        only: Option<Vec<rd_core::CandidateId>>,
        reply: Reply<Vec<(rd_core::LinkCandidate, rd_core::LinkCandidateState)>>,
    },
    FinishPackageEnqueue {
        id: rd_core::CollectorPackageId,
        success: bool,
        restore: Vec<(rd_core::CandidateId, rd_core::LinkCandidateState)>,
        reply: Reply<()>,
    },
    UpdatePackages {
        ids: Vec<rd_core::PackageId>,
        change: crate::package_store::PackageChange,
        reply: Reply<Vec<rd_core::DownloadPackage>>,
    },
    RenamePackageDirectory {
        id: rd_core::PackageId,
        name: String,
        destination: String,
        reply: Reply<Option<rd_core::DownloadPackage>>,
    },
    ClearPreviousDestination {
        id: rd_core::PackageId,
        reply: Reply<()>,
    },
    ReorderPackages {
        ids: Vec<rd_core::PackageId>,
        reply: Reply<()>,
    },
    ReorderDownloads {
        package_id: rd_core::PackageId,
        ids: Vec<rd_core::DownloadId>,
        reply: Reply<()>,
    },
    DeleteCandidate {
        id: rd_core::CandidateId,
        reply: Reply<()>,
    },
    DeleteCandidates {
        reply: Reply<u64>,
    },
    AddNzbImport {
        import: NewNzbImport,
        reply: Reply<rd_core::NzbImport>,
    },
    RecordNzbImportFailure {
        failure: crate::nzb_store::FailedNzbImport,
        reply: Reply<rd_core::NzbImport>,
    },
    UpdateNzbImport {
        id: rd_core::NzbImportId,
        change: crate::nzb_store::NzbImportChange,
        reply: Reply<rd_core::NzbImport>,
    },
    DeleteNzbImport {
        id: rd_core::NzbImportId,
        reply: Reply<()>,
    },
    ForgetNzbImportHistory {
        package_id: rd_core::PackageId,
        reply: Reply<()>,
    },
    SetNzbSegmentState {
        id: rd_core::NzbSegmentId,
        state: rd_core::NzbSegmentState,
        crc32: Option<u32>,
        reply: Reply<()>,
    },
    CheckpointNzb {
        checkpoint: crate::postprocess_store::NzbCheckpoint,
        reply: Reply<()>,
    },
    SetPackageState {
        id: rd_core::PackageId,
        state: rd_core::PackageState,
        stage: Option<rd_core::PostprocessStage>,
        percent: Option<u8>,
        current: Option<String>,
        reply: Reply<()>,
    },
    SetPackageExtraction {
        id: rd_core::PackageId,
        result: Option<rd_core::ExtractionResult>,
        reply: Reply<()>,
    },
    UpdateCategoryPostprocess {
        id: rd_core::CategoryId,
        postprocess: crate::CategoryPostprocess,
        reply: Reply<rd_core::Category>,
    },
    EnqueueNzbImport {
        id: rd_core::NzbImportId,
        destination: std::path::PathBuf,
        priority: rd_core::DownloadPriority,
        /// Creates every download row of the package paused instead of queued.
        start_paused: bool,
        reply: Reply<rd_core::PackageId>,
    },
    CreateCaptureToken {
        id: rd_core::CaptureTokenId,
        label: String,
        token_sha256: String,
        scopes: Vec<String>,
        reply: Reply<rd_core::CaptureToken>,
    },
    UpdateCaptureTokenScopes {
        id: rd_core::CaptureTokenId,
        scopes: Vec<String>,
        reply: Reply<rd_core::CaptureToken>,
    },
    RevokeCaptureToken {
        id: rd_core::CaptureTokenId,
        reply: Reply<()>,
    },
    CreateSession {
        id: rd_core::SessionId,
        token_sha256: String,
        user_agent: Option<String>,
        client_ip: Option<String>,
        lifetime_hours: i64,
        reply: Reply<rd_core::Session>,
    },
    TouchSession {
        token_sha256: String,
        reply: Reply<bool>,
    },
    RevokeSession {
        id: rd_core::SessionId,
        reply: Reply<bool>,
    },
    RevokeOtherSessions {
        keep_digest: String,
        reply: Reply<u64>,
    },
    RevokeAllSessions {
        reply: Reply<u64>,
    },
    PurgeExpiredSessions {
        limits: rd_core::SessionLimits,
        reply: Reply<u64>,
    },
    PurgeOldEvents {
        reply: Reply<u64>,
    },
    TouchCaptureToken {
        token_sha256: String,
        reply: Reply<()>,
    },
    CreateMfaCredential {
        id: rd_core::MfaCredentialId,
        kind: rd_core::MfaKind,
        label: String,
        material_ref: String,
        reply: Reply<rd_core::MfaCredential>,
    },
    ConfirmMfaCredential {
        id: rd_core::MfaCredentialId,
        reply: Reply<bool>,
    },
    TouchMfaCredential {
        id: rd_core::MfaCredentialId,
        reply: Reply<()>,
    },
    AcceptTotpStep {
        id: rd_core::MfaCredentialId,
        step: i64,
        reply: Reply<bool>,
    },
    RepointMfaMaterial {
        id: rd_core::MfaCredentialId,
        material_ref: String,
        reply: Reply<bool>,
    },
    DeleteMfaCredential {
        id: rd_core::MfaCredentialId,
        reply: Reply<Option<String>>,
    },
    ReplaceRecoveryCodes {
        digests: Vec<String>,
        reply: Reply<()>,
    },
    SpendRecoveryCode {
        digest: String,
        reply: Reply<bool>,
    },
    ClearMfa {
        kind: rd_core::MfaKind,
        reply: Reply<Vec<String>>,
    },
    CreateStorageRoot {
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
        reply: Reply<StorageRootConfig>,
    },
    UpsertNotificationTarget {
        id: Option<rd_core::NotificationTargetId>,
        input: crate::notify_store::NewNotificationTarget,
        reply: Reply<rd_notify::NotificationTarget>,
    },
    DeleteNotificationTarget {
        id: rd_core::NotificationTargetId,
        /// The vault reference of the removed target, so the caller can drop the secret.
        reply: Reply<Option<String>>,
    },
    UpsertAutomation {
        id: Option<rd_core::AutomationId>,
        input: crate::automation_store::NewAutomation,
        reply: Reply<rd_automation::Automation>,
    },
    SetAutomationEnabled {
        id: rd_core::AutomationId,
        enabled: bool,
        reply: Reply<rd_automation::Automation>,
    },
    DeleteAutomation {
        id: rd_core::AutomationId,
        reply: Reply<()>,
    },
    QueueAutomationRun {
        input: crate::automation_store::NewRun,
        reply: Reply<bool>,
    },
    RecordAutomationAttempt {
        id: rd_core::AutomationRunId,
        state: rd_automation::RunState,
        action_index: u32,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        message: Option<String>,
        reply: Reply<()>,
    },
    RecoverAutomationRuns {
        reply: Reply<u64>,
    },
    UpsertNotificationRule {
        id: Option<rd_core::NotificationRuleId>,
        input: crate::notify_store::NewNotificationRule,
        reply: Reply<rd_notify::NotificationRule>,
    },
    DeleteNotificationRule {
        id: rd_core::NotificationRuleId,
        reply: Reply<()>,
    },
    QueueNotificationDelivery {
        input: crate::notify_store::NewDelivery,
        reply: Reply<bool>,
    },
    RecordNotificationAttempt {
        id: rd_core::NotificationDeliveryId,
        state: rd_notify::DeliveryState,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        response_status: Option<u16>,
        response_excerpt: Option<String>,
        reply: Reply<()>,
    },
    /// Empties the delivery history and reports how many rows went; what the worker still
    /// owes an attempt stays (RD-130-08).
    ClearNotificationDeliveries {
        reply: Reply<u64>,
    },
    CreateBandwidthProfile {
        input: crate::bandwidth_store::NewBandwidthProfile,
        reply: Reply<rd_limits::BandwidthProfile>,
    },
    UpdateBandwidthProfile {
        id: rd_core::BandwidthProfileId,
        input: crate::bandwidth_store::NewBandwidthProfile,
        reply: Reply<rd_limits::BandwidthProfile>,
    },
    DeleteBandwidthProfile {
        id: rd_core::BandwidthProfileId,
        reply: Reply<()>,
    },
    ReplaceBandwidthWindows {
        windows: Vec<crate::bandwidth_store::NewScheduleWindow>,
        reply: Reply<Vec<rd_limits::ScheduleWindow>>,
    },
    StoreBandwidthBudget {
        profile_id: rd_core::BandwidthProfileId,
        state: rd_limits::BudgetState,
        reply: Reply<()>,
    },
    CreateCategory {
        input: NewCategory,
        reply: Reply<Category>,
    },
    CreateCategoryRule {
        input: NewCategoryRule,
        reply: Reply<CategoryRule>,
    },
    CreateHotFolder {
        input: NewHotFolder,
        reply: Reply<HotFolderConfig>,
    },
    UpdateStorageRoot {
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
        reply: Reply<StorageRootConfig>,
    },
    DeleteStorageRoot {
        id: rd_core::StorageRootId,
        reply: Reply<()>,
    },
    UpdateCategory {
        id: rd_core::CategoryId,
        input: NewCategory,
        reply: Reply<Category>,
    },
    DeleteCategory {
        id: rd_core::CategoryId,
        reply: Reply<()>,
    },
    UpdateCategoryRule {
        id: rd_core::CategoryRuleId,
        input: NewCategoryRule,
        reply: Reply<CategoryRule>,
    },
    DeleteCategoryRule {
        id: rd_core::CategoryRuleId,
        reply: Reply<()>,
    },
    UpdateHotFolder {
        id: rd_core::HotFolderId,
        input: NewHotFolder,
        reply: Reply<HotFolderConfig>,
    },
    DeleteHotFolder {
        id: rd_core::HotFolderId,
        reply: Reply<()>,
    },
    /// Writes a user site rule, replacing an earlier one of the same id (RD-110-04).
    UpsertSiteRule {
        input: crate::NewUserSiteRule,
        reply: Reply<crate::UserSiteRule>,
    },
    /// Removes a user site rule; answers whether one was there.
    DeleteSiteRule {
        id: String,
        reply: Reply<bool>,
    },
    /// Writes the results of one rule self-test run (RD-110-09).
    /// Switches one shipped rule or one group off or on (RD-110-08).
    SetSiteRuleSwitch {
        scope: String,
        key: String,
        enabled: bool,
        reply: Reply<()>,
    },
    RecordSiteRuleChecks {
        checks: Vec<crate::NewSiteRuleCheck>,
        reply: Reply<()>,
    },
    SetDownloadAuthProfile {
        id: rd_core::DownloadId,
        selection: rd_core::AuthProfileSelection,
        reply: Reply<()>,
    },
    CreateAuthProfile {
        input: crate::auth_profile_store::NewAuthProfile,
        reply: Reply<rd_core::AuthProfile>,
    },
    UpdateAuthProfile {
        id: rd_core::AuthProfileId,
        input: crate::auth_profile_store::UpdateAuthProfile,
        /// The profile plus the secret references it stopped using.
        reply: Reply<(rd_core::AuthProfile, Vec<String>)>,
    },
    SetAuthProfileEnabled {
        id: rd_core::AuthProfileId,
        enabled: bool,
        reply: Reply<rd_core::AuthProfile>,
    },
    DeleteAuthProfile {
        id: rd_core::AuthProfileId,
        /// Secret references orphaned by the deletion.
        reply: Reply<Vec<String>>,
    },
    CreateRemoteCredential {
        input: Box<crate::remote_store::NewRemoteCredential>,
        reply: Reply<rd_core::RemoteCredential>,
    },
    UpdateRemoteCredential {
        id: rd_core::RemoteCredentialId,
        input: Box<crate::remote_store::UpdateRemoteCredential>,
        /// The credential plus the secret references it stopped using.
        reply: Reply<(rd_core::RemoteCredential, Vec<String>)>,
    },
    DeleteRemoteCredential {
        id: rd_core::RemoteCredentialId,
        /// Secret references orphaned by the deletion.
        reply: Reply<Vec<String>>,
    },
    TrustSshHostKey {
        key: Box<rd_core::SshHostKey>,
        reply: Reply<()>,
    },
    ForgetSshHostKey {
        host: String,
        port: u16,
        algorithm: String,
        reply: Reply<()>,
    },
    SetCandidateListing {
        id: rd_core::CandidateId,
        listing: Box<rd_core::RemoteListing>,
        credential_id: Option<rd_core::RemoteCredentialId>,
        reply: Reply<()>,
    },
    SetCandidateListingPlan {
        id: rd_core::CandidateId,
        plan: rd_core::RemoteListingPlan,
        reply: Reply<rd_core::ResolvedRemoteListing>,
    },
    CreateStreamChannel {
        input: crate::stream_store::NewStreamChannel,
        reply: Reply<rd_core::StreamChannel>,
    },
    UpdateStreamChannel {
        id: rd_core::StreamChannelId,
        input: crate::stream_store::NewStreamChannel,
        reply: Reply<rd_core::StreamChannel>,
    },
    DeleteStreamChannel {
        id: rd_core::StreamChannelId,
        reply: Reply<()>,
    },
    TouchStreamChannel {
        id: rd_core::StreamChannelId,
        live_at: Option<chrono::DateTime<chrono::Utc>>,
        error: Option<String>,
        reply: Reply<()>,
    },
    /// One bounded pass of the transfer-statistics retention sweep (RD-110-01).
    PruneTransferStats {
        retention: crate::StatsRetention,
        reply: Reply<crate::StatsPruneReport>,
    },
    /// Stores a batch of already-redacted log records (RD-110-02).
    AppendLogRecords {
        records: Vec<crate::NewLogRecord>,
        reply: Reply<u64>,
    },
    /// Removes at most `batch` log records that retention no longer keeps.
    PruneLogRecords {
        max_records: u64,
        older_than: Option<chrono::DateTime<chrono::Utc>>,
        batch: u64,
        reply: Reply<crate::LogPruneReport>,
    },
    /// Appends audit records (RD-110-03). There is deliberately no command that updates one.
    AppendAuditRecords {
        records: Vec<crate::NewAuditRecord>,
        reply: Reply<u64>,
    },
    /// Removes at most `batch` whole audit records that retention no longer keeps.
    PruneAuditRecords {
        max_records: u64,
        older_than: Option<chrono::DateTime<chrono::Utc>>,
        batch: u64,
        reply: Reply<crate::AuditPruneReport>,
    },
    /// Empties the log store on request and reports how many records went (RD-120-34).
    ClearLogRecords {
        reply: Reply<u64>,
    },
    /// Empties the audit log and writes `record` into it as the first new entry.
    ///
    /// The record travels with the command rather than being appended afterwards because the
    /// two must commit together: an audit log that is empty with nothing saying why has lost
    /// the one trace that explains it.
    ClearAuditRecords {
        record: Box<crate::NewAuditRecord>,
        reply: Reply<u64>,
    },
    /// Empties both statistics tables and reports how many rows went.
    ClearTransferStats {
        reply: Reply<u64>,
    },
}
