//! Shared domain contracts for rDownloader.

mod audit;
mod auth_flow;
mod auth_profile;
mod bandwidth;
mod capture;
mod collector;
mod cookie_file;
mod diagnostics;
mod download;
mod error;
mod event;
pub mod failpoint;
mod gallery;
mod hotfolder;
mod ids;
mod media;
mod mfa;
mod network;
mod postprocess;
mod recording;
mod redact;
mod remote;
mod remote_job;
mod request_template;
mod session;
mod settings;
mod storage;
mod stream;
mod stream_schedule;
mod subscription;
mod toolpath;
mod torrent;
mod trace;
mod transform;
mod usenet;

pub use audit::{
    AUDIT_RETENTION_DAYS_RANGE, AUDIT_RETENTION_RECORDS_RANGE, AUDIT_TOKEN_USE_INTERVAL_SECONDS,
    AuditAction, AuditActorKind, AuditOutcome, AuditRetentionSettings,
    DEFAULT_AUDIT_RETENTION_DAYS, DEFAULT_AUDIT_RETENTION_RECORDS,
};
pub use auth_flow::{AuthFlow, AuthFlowState};
pub use auth_profile::{
    AuthMethod, AuthOrigin, AuthProfile, AuthProfileSelection, AuthScope, MAX_AUTH_CERTIFICATE,
    MAX_AUTH_COOKIES, MAX_AUTH_SECRET, ScopeError,
};
pub use bandwidth::{BandwidthSettings, DEFAULT_BANDWIDTH_TIMEZONE};
pub use capture::{
    API_ADMIN_SCOPE, API_CONFIG_SCOPE, API_INTAKE_SCOPE, API_METRICS_SCOPE, API_QUEUE_SCOPE,
    API_READ_SCOPE, API_SCOPE, API_SECRETS_SCOPE, CAPTURE_CONTRACT_VERSION, CAPTURE_SCOPE,
    CAPTURED_HEADER_ALLOWLIST, CaptureToken, CapturedHeader, CapturedRequest, MAX_CAPTURE_LINKS,
    MAX_CAPTURED_HEADER_NAME, MAX_CAPTURED_HEADERS, MAX_CAPTURED_VALUE, Scope, granted_scopes,
    is_allowed_captured_header, is_credential_header, scope_satisfies, scopes_grant,
    scopes_satisfy,
};
pub use collector::{
    CandidateMessage, CandidateMirror, Category, CategoryRule, CollectorBatch, CollectorPackage,
    EnrichmentField, GrabberEntryKind, GrabberEntryRef, HotFolderConfig, HotFolderExecutor,
    ImportMode, IngressSource, LinkCandidate, LinkCandidateState, LinkCheckResult, LinkStatus,
    MirrorFacet, MirrorHint, MirrorPreference, MirrorSource, StorageRootConfig, candidate_url,
    split_candidate_url,
};
pub use cookie_file::{
    CookieFileError, CookieRow, MAX_COOKIE_FILE, earliest_expiry as cookie_earliest_expiry,
    parse as parse_cookie_file, to_netscape_file,
};
pub use diagnostics::{
    DEFAULT_LOG_RETENTION_DAYS, DEFAULT_LOG_RETENTION_RECORDS, LOG_RETENTION_DAYS_RANGE,
    LOG_RETENTION_RECORDS_RANGE, LogLevel, LogRetentionSettings,
};
pub use download::{
    ByteCount, ChecksumAlgorithm, DownloadFile, DownloadKind, DownloadPackage, DownloadPriority,
    DownloadState, ExpectedChecksum, is_recovery_volume,
};
pub use error::{Failure, FailureKind, MessageParams};
pub use event::{EventEnvelope, EventKind};
pub use gallery::{GALLERY_PROVIDER, GallerySettings};
pub use hotfolder::{
    DEFAULT_HOTFOLDER_POLL_SECONDS, HOTFOLDER_POLL_SECONDS_RANGE, HotFolderSettings,
};
pub use ids::{
    AccountId, AuthProfileId, AutomationId, AutomationRunId, AutomationVersionId,
    BandwidthProfileId, BandwidthWindowId, BatchId, CandidateId, CaptchaId, CaptureAgentId,
    CaptureTokenId, CategoryId, CategoryRuleId, ChunkId, CollectorPackageId, DownloadId, EventId,
    HotFolderId, MfaCredentialId, NotificationDeliveryId, NotificationRuleId, NotificationTargetId,
    NzbFileId, NzbImportId, NzbSegmentId, PackageId, PluginId, ProxyProfileId, RemoteCredentialId,
    RemoteJobId, SessionId, StorageRootId, StreamChannelId, StreamScheduleId, StreamScheduledRunId,
    SubscriptionId, SubscriptionItemId, SubscriptionRunId, UsenetServerId,
};
pub use media::{
    AudioCodecFamily, AudioTrack, AudioTrackPolicy, CONTAINERS, ContainerCapabilities,
    CriteriaError, CriterionKind, CriterionMatch, DynamicRange, EmbedWarning, LEGACY_PRESETS,
    MAX_CRITERIA_TOKEN, MAX_CRITERIA_VALUES, MAX_MEDIA_FORMATS, MAX_MEDIA_TRACKS,
    MEDIA_CONTRACT_VERSION, MEDIA_PROVIDER, MediaCandidate, MediaCandidateState,
    MediaCompatibilityWarning, MediaEmbedPolicy, MediaFormat, MediaFormatCriteria,
    MediaFormatInventory, MediaFormatKind, MediaInfo, MediaKind, MediaOutput, MediaResolution,
    MediaSelection, MediaSelectionError, MediaSelectionUpdate, MediaSettings, MediaStrictness,
    MediaTarget, MediaVariant, RELAXATION_ORDER, ResolvedFormatPlan, SponsorBlockPolicy,
    SponsorCategory, SponsorMode, SubtitleMode, SubtitlePolicy, SubtitleSource, SubtitleTrack,
    TrackSelection, TrackWarning, VideoCodecFamily, capabilities, effective_policy, embed_warnings,
    is_audio_only, is_criteria_token, supports_audio_codec, supports_chapters,
    supports_multiple_audio, supports_subtitles, supports_thumbnail, supports_video_codec,
    track_warnings,
};
pub use mfa::{MfaCredential, MfaKind, MfaStatus};
pub use network::{Account, ProxyKind, ProxyProfile, ResolverPin, ResolverRoute};
pub use postprocess::{
    ExtractionResult, PackageState, PostprocessHold, PostprocessHoldGuard, PostprocessLevel,
    PostprocessStage, PostprocessStatus, is_par2_index, is_par2_volume, par2_volume_belongs_to,
    par2_volume_blocks,
};
pub use recording::{
    MAX_RECONNECTS, MAX_SEGMENTS, MAX_SPLIT_MINUTES, MIN_SPLIT_MEGABYTES, MIN_SPLIT_MINUTES,
    RecordingPolicy, RecordingSegment, RecordingState, RemuxTarget, SegmentEnd, SidecarOutcome,
    SidecarPolicy, SidecarStatus, SplitPolicy, VodFallback, segment_name,
};
pub use redact::{
    REDACTION_PLACEHOLDER, Redacted, SIGNED_QUERY_MARKERS, SIGNED_QUERY_SECRETS,
    is_secret_parameter, is_signed_url, redact_failure, redact_header_value, redact_params,
    redact_text, redact_url, signed_url_expiry,
};
pub use remote::{
    FTP_PROVIDER, ListingLimit, MAX_REMOTE_DEPTH, MAX_REMOTE_ENTRIES, MAX_REMOTE_HOST,
    MAX_REMOTE_KEY, MAX_REMOTE_PATH, MAX_REMOTE_SECRET, REMOTE_CONTRACT_VERSION, RemoteAuthMode,
    RemoteCandidateState, RemoteCredential, RemoteEntry, RemoteFamily, RemoteListing,
    RemoteListingPlan, RemoteListingSummary, RemoteProtocol, RemoteSettings, RemoteTarget,
    ResolvedRemoteEntry, ResolvedRemoteListing, SFTP_PROVIDER, SshHostKey, WEBDAV_PROVIDER,
    is_safe_relative_path, resolve_listing,
};
pub use remote_job::{
    MAX_POLL_SECONDS, MAX_SUBMIT_ATTEMPTS, MIN_POLL_SECONDS, RemoteJob, RemoteJobFile,
    RemoteJobSourceKind, RemoteJobState, SubmitStep,
};
pub use request_template::{
    CapturedBody, CredentialCategory, MAX_APPROVED_ORIGINS, MAX_BODY_FIELD_NAMES,
    MAX_REPLAY_BODY_B64, MAX_REPLAY_BODY_BYTES, REQUEST_TEMPLATE_VERSION, ReplayBlockReason,
    ReplayBodyKind, ReplayConsent, ReplayMethod, ReplaySummary, RequestTemplate,
    credential_categories, derive_approved_origins, needs_consent, origin_of, stable_hash,
};
pub use session::{
    DEFAULT_SESSION_IDLE_HOURS, DEFAULT_SESSION_MAX_HOURS, MAX_USER_AGENT,
    SESSION_IDLE_HOURS_RANGE, SESSION_MAX_HOURS_RANGE, Session, SessionLimits, truncate_user_agent,
};
pub use settings::{PostprocessSettings, ServiceSwitches};
pub use storage::{
    DEFAULT_MINIMUM_FREE_BYTES, DEFAULT_UNKNOWN_SIZE_HEADROOM, MAX_UNKNOWN_SIZE_HEADROOM,
    StorageSettings,
};
pub use stream::{RECORD_PROVIDER, StreamChannel, StreamSettings};
pub use stream_schedule::{
    MAX_ROLL_MINUTES, MAX_WINDOW_MINUTES, MINUTES_PER_DAY, PLANNING_HORIZON_DAYS, ScheduleError,
    ScheduleKind, ScheduledRunState, StreamSchedule, StreamScheduledRun,
};
pub use subscription::{
    BacklogPolicy, CategoryMapping, DEFAULT_POLL_INTERVAL_SECONDS, FilterReason,
    MAX_CATEGORY_MAPPINGS, MAX_FILTER_PATTERNS, MAX_ITEM_KEY, MAX_ITEMS_PER_POLL,
    MAX_POLL_INTERVAL_SECONDS, MIN_POLL_INTERVAL_SECONDS, SCRIPT_URL_SCHEME,
    SITE_RULE_MIN_POLL_INTERVAL_SECONDS, Subscription, SubscriptionBulkStateResponse,
    SubscriptionCardRatio, SubscriptionFilters, SubscriptionHistoryClearResponse, SubscriptionItem,
    SubscriptionItemCounts, SubscriptionItemPage, SubscriptionItemState, SubscriptionKind,
    SubscriptionMode, SubscriptionReviewCount, SubscriptionReviewSummary, SubscriptionRun,
    SubscriptionSettings, SubscriptionView,
};
pub use toolpath::{
    ManagedTool, ManagedToolResolver, ManagedToolSettings, ResolvedTool, ToolLease, ToolSource,
    VENDOR_DIR_NAME, data_directory, executable_in, locate_tool, locate_tool_leased, managed_tool,
    set_data_directory, set_managed_tool_resolver, vendor_directories,
};
pub use torrent::{
    DEFAULT_PEER_PAGE, EffectiveSeedingPolicy, MAX_EXCLUSION_PATTERN_LENGTH,
    MAX_EXCLUSION_PATTERNS, MAX_PEER_PAGE, MAX_SEED_RATIO, MAX_SEED_TIME_MINUTES,
    MAX_TORRENT_TRACKERS, MAX_TRACKER_URL, MIN_SEED_RATIO, PIECE_BUCKETS, PolicySource,
    ResolvedTorrentPlan, SeedAccounting, SeedTimeLimit, SeedingPolicyOverride,
    TORRENT_CONTENT_TYPES, TORRENT_CONTRACT_VERSION, TORRENT_PROVIDER,
    TRACKER_REDACTION_PLACEHOLDER, TorrentAggregateStats, TorrentCandidateState,
    TorrentCandidateSummary, TorrentEngineCapabilities, TorrentFileDecision, TorrentFileEntry,
    TorrentFilePlan, TorrentFilePriority, TorrentJobState, TorrentListenMode, TorrentMetadataInfo,
    TorrentMetadataState, TorrentPeerEntry, TorrentPeerPage, TorrentPieceAvailability,
    TorrentSequentialMode, TorrentSettings, TorrentTracker, TrackerOrigin, TrackerScrape,
    glob_match, mask_peer_address, redact_tracker_url, resolve_plan, resolve_seeding_policy,
    tracker_id,
};
pub use trace::{
    DEFAULT_OTLP_TIMEOUT_SECONDS, OTLP_TIMEOUT_SECONDS_RANGE, OtelSettings, TRACE_FIELD,
    TraceContext, is_valid_otlp_endpoint,
};
pub use transform::{
    BLOCK_BYTES, CIPHER_AES_128_CTR, CODE_CHECKPOINT_MISMATCH, CODE_CIPHER_UNKNOWN,
    CODE_INTEGRITY_MISMATCH, CODE_INTEGRITY_UNKNOWN, CODE_KEY_MISSING, CODE_PARAMETERS_INVALID,
    CipherSpec, ContentTransform, INTEGRITY_CBC_MAC_CHAIN, IntegritySpec, MAX_BOUNDARIES,
    TransformKey,
};
pub use usenet::{
    NZB_CONTENT_TYPES, NZB_PROVIDER, NzbFileStatus, NzbImport, NzbImportState, NzbSegmentState,
    NzbSegmentStatus, PostprocessKind, PostprocessState, PostprocessStep, UsenetServer,
    provider_for_media_type,
};

/// Maximum representable byte count in persistent storage.
pub const MAX_PERSISTED_BYTES: u64 = i64::MAX as u64;

/// Serde default for a flag whose absence means "yes".
///
/// Used where a field was added to a type that is already persisted: a stored value written
/// before the field existed has to read back as the behaviour that was in force then.
pub(crate) const fn default_true() -> bool {
    true
}
