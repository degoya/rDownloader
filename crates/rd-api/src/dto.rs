use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Whether initial setup has been completed.
#[derive(Serialize, ToSchema)]
pub struct AuthStatus {
    pub setup_required: bool,
    pub authenticated: bool,
    /// The administrator login is switched off in the settings; every request is trusted.
    #[serde(default)]
    pub login_disabled: bool,
    /// Whether a passkey is enrolled, so the sign-in screen knows to offer it.
    #[serde(default)]
    pub passkeys_available: bool,
}

/// First-run wizard progress plus what the install already has configured.
#[derive(Serialize, ToSchema)]
pub struct SetupStatus {
    /// Explicit completion flag, or a storage root already exists.
    pub wizard_completed: bool,
    pub storage_roots: u32,
    /// Of those, how many sit on a path that a container restart wipes.
    pub ephemeral_storage_roots: u32,
    pub categories: u32,
    pub capture_agents: u32,
    pub accounts: u32,
    pub usenet_servers: u32,
    /// The service's own download directory, offered as the first storage root.
    ///
    /// The form used to start on a hardcoded `/downloads`, which is right inside the container
    /// image and wrong on every native install — on Linux the filesystem root belongs to another
    /// user, so accepting the suggestion produced a failure. Suggesting the directory the service
    /// actually writes to means the offered value is one it has already created.
    pub suggested_storage_path: String,
}

/// Initial administrator password.
#[derive(Deserialize, ToSchema)]
pub struct SetupRequest {
    #[schema(write_only)]
    pub password: String,
}

/// Administrator login request.
#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    #[schema(write_only)]
    pub password: String,
    /// A code from an authenticator app, or a recovery code.
    ///
    /// Optional so the first request can omit it: the client learns that a second factor is
    /// required from the `auth.mfa_required` refusal, rather than having to ask in advance
    /// whether this installation uses one. Asking in advance would tell an unauthenticated
    /// caller something about the account.
    #[serde(default)]
    pub code: Option<String>,
}

/// A change of the administrator password by somebody who knows the current one (RD-120-22).
///
/// Both fields are `write_only`: neither may ever appear in a response, and the generated
/// schema is what keeps a future handler from echoing one back.
#[derive(Deserialize, ToSchema)]
pub struct PasswordChangeRequest {
    /// The password in force now. Wrong is refused exactly as a wrong sign-in is.
    #[schema(write_only)]
    pub current_password: String,
    /// The replacement, judged by the same policy the first password was.
    #[schema(write_only)]
    pub new_password: String,
}

/// Human label for a newly paired native capture agent.
#[derive(Deserialize, ToSchema)]
pub struct CapturePairRequest {
    pub label: String,
}

/// Pairing request for a machine API token.
///
/// Separate from [`CapturePairRequest`] because a capture agent has no scope choice: it
/// always gets `capture:*`, while an API client picks between full access and read-only.
#[derive(Deserialize, ToSchema)]
pub struct ApiTokenRequest {
    pub label: String,
    /// The areas this token may reach, as scope strings.
    ///
    /// Empty falls back to [`Self::read_only`], which is what an older client sends. A caller
    /// that names scopes gets exactly those: nothing here widens a request, because a minting
    /// call that quietly grants more than it was asked for is the one mistake this whole model
    /// exists to prevent.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Legacy: mint `api:read` instead of `api:*`. Superseded by [`Self::scopes`].
    ///
    /// Kept because it shipped, and because an existing client that only sends a label must
    /// keep getting the token it used to get. Ignored when `scopes` is non-empty.
    #[serde(default)]
    pub read_only: bool,
}

/// The new set of areas for a token that already exists.
///
/// Only the areas: no label, and above all no bearer. Re-scoping is not a re-issue, and a DTO
/// that could carry a new secret would invite one.
#[derive(Deserialize, ToSchema)]
pub struct ApiTokenScopesRequest {
    /// The areas the token holds from now on, as scope strings — the complete set, not a delta.
    ///
    /// A replacement rather than an add/remove pair because the caller sends what it sees in
    /// the form, and two clients editing the same token should not silently merge their
    /// intentions. Empty is refused: it has no legacy meaning here, and "no access at all" is
    /// spelled by revoking the token.
    pub scopes: Vec<String>,
}

/// One area a token can be granted, with what it costs and what it reaches.
///
/// Sent to the token editor so the choice is made against real numbers. Everything here is
/// derived from the same table that enforces the scopes, so the preview cannot drift away
/// from the behaviour it is previewing.
#[derive(Debug, Serialize, ToSchema)]
pub struct ScopeDescriptor {
    /// The scope string, e.g. `api:queue`.
    pub scope: String,
    /// What holding this scope also confers, e.g. `api:read` for anything that acts.
    pub implies: Vec<String>,
    /// How many API operations a token holding exactly this scope reaches, implications
    /// included.
    pub operations: u32,
    /// Whether this area carries stored credentials or service administration.
    ///
    /// Flagged rather than left to the label: these are the two areas nothing else confers,
    /// and the two whose consequences are worst to grant by accident.
    pub sensitive: bool,
}

/// One-time bearer plus its revocable metadata.
#[derive(Serialize, ToSchema)]
pub struct CapturePairResponse {
    pub bearer: String,
    pub token: rd_core::CaptureToken,
}

/// Action result: English text plus a stable code (and parameters) clients translate.
#[derive(Serialize, ToSchema)]
pub struct MessageResponse {
    pub message: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: crate::error::MessageParams,
}

impl MessageResponse {
    /// Creates a coded action result.
    #[must_use]
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.to_owned(),
            params: Default::default(),
        }
    }

    /// Attaches a translation parameter.
    #[must_use]
    pub fn with_param(mut self, key: &str, value: impl ToString) -> Self {
        self.params.insert(key.to_owned(), value.to_string());
        self
    }

    /// Attaches the `count` parameter used by pluralised messages.
    #[must_use]
    pub fn with_count(self, count: impl ToString) -> Self {
        self.with_param("count", count)
    }
}

/// Redaction-safe result of a live provider account check.
#[derive(Serialize, ToSchema)]
pub struct AccountTestResponse {
    pub valid: bool,
    pub premium: bool,
    /// What the interface prints next to the account, one translated part after another,
    /// joined with a separator of its own. Empty when the check has nothing to add to the flags.
    pub label: Vec<AccountLabelPart>,
    pub traffic_left: Option<rd_core::ByteCount>,
}

/// One translatable part of an account label, in the shape of every coded server message:
/// `code` is looked up in the active language, then in English, `params` are interpolated,
/// and `message` is printed only when no catalogue knows the code.
#[derive(Serialize, ToSchema)]
pub struct AccountLabelPart {
    /// `plugin.account.*` from the core catalogue or `<provider.slug>.*` from the plugin's own.
    pub code: String,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: std::collections::BTreeMap<String, String>,
    /// English, redaction-safe text for a code no catalogue translates.
    pub message: String,
}

impl From<rd_plugin_host::LabelPart> for AccountLabelPart {
    fn from(part: rd_plugin_host::LabelPart) -> Self {
        Self {
            code: part.code,
            params: part.params,
            message: part.message,
        }
    }
}

/// Direct URL queue request.
#[derive(Deserialize, ToSchema)]
pub struct CreateDownloadRequest {
    #[schema(format = "uri")]
    pub url: String,
    pub package_name: Option<String>,
    pub file_name: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    pub account_id: Option<rd_core::AccountId>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub priority: Option<rd_core::DownloadPriority>,
}

/// Text or URL intake for the LinkGrabber.
#[derive(Deserialize, ToSchema)]
pub struct CollectorIntakeRequest {
    /// Free text the server scans for links; optional when `links` is used.
    #[serde(default)]
    pub text: Option<String>,
    pub source: rd_core::IngressSource,
    pub source_label: Option<String>,
    /// Explicit package name (Click'n'Load package or manual input); keeps all links together.
    pub package_name: Option<String>,
    /// Archive password announced with the links.
    ///
    /// Readable again on the package (RD-104-04); see `DownloadPackage::password`.
    pub password: Option<String>,
    /// Structured links with per-link request metadata (capture contract v1).
    #[serde(default)]
    pub links: Vec<CaptureLinkRequest>,
}

/// One structured link of a capture batch, optionally with the request that produced it.
#[derive(Deserialize, ToSchema)]
pub struct CaptureLinkRequest {
    #[schema(format = "uri")]
    pub url: String,
    pub file_name: Option<String>,
    /// Metadata to reproduce the GET; sanitized and allowlisted on arrival.
    #[serde(default)]
    pub request: Option<rd_core::CapturedRequest>,
}

/// Result of one intake: the batch, its packages and links.
#[derive(Serialize, ToSchema)]
pub struct CollectorIntakeResponse {
    pub batch: rd_core::CollectorBatch,
    pub packages: Vec<rd_core::CollectorPackage>,
    pub candidates: Vec<rd_core::LinkCandidate>,
    /// Links dropped by the domain blocklist before candidates were created.
    pub skipped_excluded: u32,
    /// Links dropped because the service that would carry them is switched off.
    pub skipped_disabled: u32,
    /// Addresses the folder crawlers and site rules handed back for this intake, before any
    /// of them was judged. Zero when no crawler ran.
    pub crawled_found: u32,
    /// How many of those were refused because nothing claims them and no probe confirmed
    /// them to be files (RD-110-07). A rule reaching one element too far shows up here
    /// rather than as a page that quietly became a download.
    pub crawled_dropped: u32,
}

/// Editable LinkGrabber package fields.
#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageUpdateRequest {
    pub name: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
    /// Archive password; readable again on the package (RD-104-04).
    pub password: Option<String>,
    #[serde(default)]
    pub clear_password: bool,
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub clear_postprocess_level: bool,
    pub script: Option<String>,
    #[serde(default)]
    pub clear_script: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageBulkRequest {
    pub ids: Vec<rd_core::CollectorPackageId>,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub clear_postprocess_level: bool,
    pub script: Option<String>,
    #[serde(default)]
    pub clear_script: bool,
}

/// Editable routing metadata of an NZB waiting in the LinkGrabber.
#[derive(Deserialize, ToSchema)]
pub struct NzbImportUpdateRequest {
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
}

/// How an NZB import enters the download queue.
///
/// The body is optional: a request without one keeps the previous behaviour and starts the
/// package immediately.
#[derive(Default, Deserialize, ToSchema)]
pub struct NzbImportEnqueueRequest {
    /// Creates every download of the package paused instead of starting it immediately.
    #[serde(default)]
    pub paused: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageReorderRequest {
    pub ids: Vec<rd_core::CollectorPackageId>,
}

/// A slice of the LinkGrabber's manual order over both kinds of entry, in display order.
///
/// A list may be partial - the client sends the rows it is showing, and filters hide rows - but
/// every entry has to name a row that exists under the kind it claims.
#[derive(Deserialize, ToSchema)]
pub struct GrabberEntryReorderRequest {
    pub entries: Vec<rd_core::GrabberEntryRef>,
    /// The entry the listed ones are placed behind; absent means the head of the list.
    ///
    /// Without it a partial list can only describe a prefix, so a drag deep into a long list has
    /// to send everything above it and runs into the bulk bound. The anchor is what keeps a drag
    /// at index 700 the same two entries as a drag at index 2.
    #[serde(default)]
    pub after: Option<rd_core::GrabberEntryRef>,
}

/// Packages to enqueue in the given (displayed) order.
#[derive(Deserialize, ToSchema)]
pub struct CollectorPackageEnqueueRequest {
    pub ids: Vec<rd_core::CollectorPackageId>,
    /// Enqueues only these links of the packages; the others stay in the LinkGrabber, in their
    /// package. What the LinkGrabber sends while a filter hides part of a package — absent, a
    /// package goes whole.
    #[serde(default)]
    pub candidate_ids: Option<Vec<rd_core::CandidateId>>,
    /// Creates every download in paused state instead of starting it immediately.
    #[serde(default)]
    pub paused: bool,
}

/// Result of a batch enqueue; partial failures are reported instead of silently dropped.
#[derive(Serialize, ToSchema)]
pub struct CollectorEnqueueBatchResponse {
    pub created: Vec<rd_core::DownloadPackage>,
    /// Packages that could not be enqueued.
    pub failed: u32,
    /// Message of the first failure, when any package failed.
    pub first_error: Option<String>,
    /// Files enqueued without a provider account (free/direct download attempt).
    pub free_download_files: u32,
}

#[derive(Deserialize, ToSchema)]
pub struct CandidateMoveRequest {
    pub ids: Vec<rd_core::CandidateId>,
    pub package_id: Option<rd_core::CollectorPackageId>,
    pub new_package_name: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CandidateReorderRequest {
    pub package_id: rd_core::CollectorPackageId,
    pub ids: Vec<rd_core::CandidateId>,
}

/// Links to check; empty = every open link.
#[derive(Deserialize, ToSchema)]
pub struct CandidateCheckRequest {
    pub ids: Option<Vec<rd_core::CandidateId>>,
}

#[derive(Deserialize, ToSchema)]
pub struct CandidateRenameRequest {
    pub file_name: Option<String>,
    /// Switches the media variant (`best`, `1080p`, `audio_mp3`, …) of a media link.
    pub media_variant: Option<String>,
}

/// Availability of one external tool.
#[derive(Serialize, ToSchema)]
pub struct MediaToolStatus {
    pub name: String,
    pub path: Option<String>,
    pub version: Option<String>,
    /// Whether the binary came from the explicit setting, the managed store, a vendor folder
    /// or `PATH`.
    pub source: Option<rd_core::ToolSource>,
    /// Whether this is a tool the application can manage itself (RD-102-02). A managed tool
    /// may still resolve to a system binary; that is what `source` says.
    pub managed: bool,
    /// The managed version currently activated, if any. Independent of `version`, which is
    /// whatever the resolved binary reports about itself.
    pub active_version: Option<String>,
    /// What the compatibility rules make of the version that was found (RD-102-03).
    pub compatibility: ToolCompatibility,
}

/// The compatibility verdict for one external tool (RD-102-03).
///
/// Four states rather than a boolean, because they call for different answers: `too_old` has
/// an upgrade, `known_bad` may call for a different version in either direction, and `unknown`
/// is an absence of information that never blocks anything.
#[derive(Serialize, ToSchema)]
pub struct ToolCompatibility {
    /// `supported`, `too_old`, `known_bad` or `unknown`.
    pub verdict: rd_tools::Verdict,
    /// The version the rules were applied to, normalised when it could be parsed and the raw
    /// line when it could not.
    pub version: Option<String>,
    /// The oldest version this build is tested against, when a rule sets one.
    pub min_version: Option<String>,
    /// What is lost while the verdict is not `supported`. Empty when no rule covers the tool.
    pub affects: Vec<rd_tools::Capability>,
    /// Whether the settings name this tool as overridden, so the verdict is reported but not
    /// enforced.
    pub overridden: bool,
    /// One English sentence saying what to do about it, or `null` when there is nothing to do.
    pub upgrade: Option<String>,
}

impl From<rd_tools::Assessment> for ToolCompatibility {
    fn from(assessment: rd_tools::Assessment) -> Self {
        Self {
            upgrade: assessment.upgrade(),
            verdict: assessment.verdict,
            version: assessment.version,
            min_version: assessment.min_version,
            affects: assessment.affects,
            overridden: assessment.overridden,
        }
    }
}

/// One managed external tool: what is installed, what is active, what is on offer.
#[derive(Serialize, ToSchema)]
pub struct ManagedToolInfo {
    /// `yt-dlp`, `gallery-dl`, `streamlink`, `ffmpeg` or `ffprobe`.
    pub name: String,
    /// The version the managed stage of the tool lookup answers with.
    pub active_version: Option<String>,
    /// The executable that version points at.
    pub active_path: Option<String>,
    /// Every version in the store, newest install first.
    pub installed_versions: Vec<String>,
    /// The newest version the signed manifest offers for this platform and this application
    /// version, or `null` when it offers none.
    pub available_version: Option<String>,
    /// Whether a rollback has an earlier installed version to return to.
    pub can_roll_back: bool,
}

/// The managed tool store as a whole.
#[derive(Serialize, ToSchema)]
pub struct ManagedToolsResponse {
    /// Whether this installation may install and activate tool versions at all.
    pub enabled: bool,
    /// The target triple manifest entries are matched against.
    pub platform: String,
    /// The sequence of the manifest currently in force. `0` is the manifest compiled into
    /// this build, before any refresh.
    pub manifest_sequence: u64,
    /// When the publisher signed that manifest.
    pub manifest_issued_at: String,
    /// The configured manifest URL, if any.
    pub manifest_url: Option<String>,
    pub tools: Vec<ManagedToolInfo>,
}

/// Which version to install or activate. `null` takes the newest the manifest offers.
#[derive(Deserialize, ToSchema)]
pub struct ManagedToolVersionRequest {
    #[serde(default)]
    pub version: Option<String>,
}

/// External-tool availability and the hosts routed to the media provider.
#[derive(Serialize, ToSchema)]
pub struct MediaStatusResponse {
    pub ytdlp: MediaToolStatus,
    pub ffmpeg: MediaToolStatus,
    /// yt-dlp needs ffprobe next to ffmpeg to merge streams and convert audio.
    pub ffprobe: MediaToolStatus,
    pub unrar: MediaToolStatus,
    pub seven_zip: MediaToolStatus,
    pub rclone: MediaToolStatus,
    pub gallery_dl: MediaToolStatus,
    pub streamlink: MediaToolStatus,
    /// The notification CLI; a target may name its own executable, this is the lookup without.
    pub apprise: MediaToolStatus,
    /// Vendor folders searched before `PATH`, in order.
    pub vendor_directories: Vec<String>,
    pub hosts: Vec<String>,
}

/// Whether writes below a storage root outlive the container.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StoragePersistence {
    /// On a mount that survives, or not in a container at all.
    Persistent,
    /// In the container's writable layer or on a memory-backed filesystem: everything below
    /// it is deleted when the container is removed or recreated.
    Ephemeral,
    /// The mount table could not be read, or the path could not be matched against it.
    Unknown,
}

impl From<rd_files::PathPersistence> for StoragePersistence {
    fn from(value: rd_files::PathPersistence) -> Self {
        match value {
            rd_files::PathPersistence::Persistent => Self::Persistent,
            rd_files::PathPersistence::Ephemeral => Self::Ephemeral,
            rd_files::PathPersistence::Unknown => Self::Unknown,
        }
    }
}

/// A storage root plus the runtime verdict on its path.
///
/// Deliberately not a field on `rd_core::StorageRootConfig`: that type is also the backup
/// type, and a machine-local verdict written into an exported bundle would mean nothing on
/// the machine that restores it.
#[derive(Serialize, ToSchema)]
pub struct StorageRootResponse {
    #[serde(flatten)]
    pub root: rd_core::StorageRootConfig,
    pub persistence: StoragePersistence,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateStorageRootRequest {
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// Free space kept on this root; empty inherits `storage_minimum_free_bytes`.
    #[serde(default)]
    pub minimum_free_bytes: Option<rd_core::ByteCount>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateCategoryRequest {
    pub name: String,
    pub color: String,
    pub storage_root_id: rd_core::StorageRootId,
    pub relative_path: String,
    pub is_default: bool,
    /// Default post-processing level for packages of this category (`null` = global default).
    #[serde(default)]
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    /// Default post-processing script (file name inside the scripts directory).
    #[serde(default)]
    pub script: Option<String>,
    /// Extensions removed after unpacking this category (`null` = global cleanup list).
    #[serde(default)]
    pub cleanup_extensions: Option<Vec<String>>,
    /// Whether packages of this category unpack nested archives recursively (`null` = global default).
    #[serde(default)]
    pub recursive_unpack: Option<bool>,
    /// Whether packages of this category verify `.sfv` checksums (`null` = global default).
    #[serde(default)]
    pub sfv_verify: Option<bool>,
    /// Whether a failed PAR2/SFV/RAR verification blocks unpacking and everything after it
    /// (`null` = global default). Off means the unpack runs anyway (RD-104-04).
    #[serde(default)]
    pub safe_postproc: Option<bool>,
    /// Whether packages of this category discard the PAR2 recovery set after a successful
    /// unpack (`null` = global default).
    #[serde(default)]
    pub delete_par2: Option<bool>,
    /// Whether packages of this category upload to rclone (`null` = global default).
    #[serde(default)]
    pub upload_enabled: Option<bool>,
    /// rclone target override in `remote:path` form (`null` = the global remote).
    #[serde(default)]
    pub upload_remote: Option<String>,
}

/// Post-processing defaults of a category (`null` = inherit the global setting).
#[derive(Deserialize, ToSchema)]
pub struct CategoryPostprocessRequest {
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    pub script: Option<String>,
    #[serde(default)]
    pub cleanup_extensions: Option<Vec<String>>,
    /// Whether packages of this category unpack nested archives recursively (`null` = global default).
    #[serde(default)]
    pub recursive_unpack: Option<bool>,
    /// Whether packages of this category verify `.sfv` checksums (`null` = global default).
    #[serde(default)]
    pub sfv_verify: Option<bool>,
    /// Whether a failed PAR2/SFV/RAR verification blocks unpacking and everything after it
    /// (`null` = global default). Off means the unpack runs anyway (RD-104-04).
    #[serde(default)]
    pub safe_postproc: Option<bool>,
    /// Whether packages of this category discard the PAR2 recovery set after a successful
    /// unpack (`null` = global default).
    #[serde(default)]
    pub delete_par2: Option<bool>,
    /// Post-processing plugin steps for this category, by plugin id and in the order they
    /// run (`null` = the global list). An empty list means "none here", which is how a
    /// category switches a globally enabled step off.
    #[serde(default)]
    pub plugin_steps: Option<Vec<String>>,
    /// Whether packages of this category upload to rclone (`null` = global default).
    #[serde(default)]
    pub upload_enabled: Option<bool>,
    /// rclone target override in `remote:path` form (`null` = the global remote).
    #[serde(default)]
    pub upload_remote: Option<String>,
}

/// Editable fields of a monitored livestream channel.
#[derive(Deserialize, ToSchema)]
pub struct StreamChannelRequest {
    #[schema(format = "uri")]
    pub url: String,
    /// Display name; defaults to the URL host. Doubles as the recording file prefix.
    pub name: Option<String>,
    /// streamlink stream selection (`best`, `1080p`, …); `null` = the default quality.
    pub quality: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default = "default_channel_enabled")]
    pub enabled: bool,
    /// Splitting, remux, sidecars and VOD fallback for this channel (RD-080-09).
    #[serde(default)]
    pub recording: rd_core::RecordingPolicy,
}

fn default_channel_enabled() -> bool {
    true
}

/// Immediate one-off recording of a livestream URL.
#[derive(Deserialize, ToSchema)]
pub struct RecordNowRequest {
    #[schema(format = "uri")]
    pub url: String,
    pub name: Option<String>,
    pub quality: Option<String>,
    pub category_id: Option<rd_core::CategoryId>,
}

/// One package currently in (or waiting for) the post-processing pipeline.
#[derive(Serialize, ToSchema)]
pub struct PostprocessQueueEntry {
    pub package_id: rd_core::PackageId,
    pub name: String,
    pub state: rd_core::PackageState,
    pub stage: Option<rd_core::PostprocessStage>,
    pub percent: Option<u8>,
    pub current: Option<String>,
    /// `true` while the job waits for the single post-processing worker.
    pub pending: bool,
}

/// Script files available in the configured scripts directory.
#[derive(Serialize, ToSchema)]
pub struct PostprocessScriptsResponse {
    pub directory: String,
    pub scripts: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateCategoryRuleRequest {
    pub name: String,
    pub priority: i32,
    pub source: Option<rd_core::IngressSource>,
    pub domain: Option<String>,
    pub protocol: Option<String>,
    pub extension: Option<String>,
    pub mime_type: Option<String>,
    pub name_regex: Option<String>,
    pub category_id: rd_core::CategoryId,
    pub enabled: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateHotFolderRequest {
    pub name: String,
    pub executor: rd_core::HotFolderExecutor,
    pub path: String,
    pub recursive: bool,
    pub category_id: Option<rd_core::CategoryId>,
    pub import_mode: rd_core::ImportMode,
    pub processed_path: String,
    pub failed_path: String,
    pub enabled: bool,
}

/// Account metadata with write-only secret material.
#[derive(Deserialize, ToSchema)]
pub struct CreateAccountRequest {
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential `secret` holds, for a provider that offers a choice. Required for
    /// those providers, rejected for every other one.
    #[serde(default)]
    pub credential_mode: Option<rd_provider_registry::CredentialMode>,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub cookies: Option<String>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub enabled: bool,
}

/// Editable account metadata. Empty secret fields preserve their stored values.
#[derive(Deserialize, ToSchema)]
pub struct UpdateAccountRequest {
    pub provider: String,
    pub label: String,
    pub username: Option<String>,
    /// Which credential `secret` holds, for a provider that offers a choice. Required for
    /// those providers, rejected for every other one.
    #[serde(default)]
    pub credential_mode: Option<rd_provider_registry::CredentialMode>,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub cookies: Option<String>,
    pub clear_secret: bool,
    pub clear_cookies: bool,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub enabled: bool,
}

/// Network proxy profile with a write-only password.
#[derive(Deserialize, ToSchema)]
pub struct CreateProxyProfileRequest {
    pub name: String,
    pub kind: rd_core::ProxyKind,
    #[schema(format = "uri")]
    pub endpoint: String,
    pub username: Option<String>,
    #[schema(write_only)]
    pub password: Option<String>,
}

/// Persistent NNTP endpoint with a write-only password.
#[derive(Deserialize, ToSchema)]
pub struct CreateUsenetServerRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    #[schema(write_only)]
    pub password: Option<String>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
}

/// Editable NNTP endpoint. An omitted password preserves the stored password.
#[derive(Deserialize, ToSchema)]
pub struct UpdateUsenetServerRequest {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    #[schema(write_only)]
    pub password: Option<String>,
    pub clear_password: bool,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
}

/// Mutable local service settings exposed in version one.
#[derive(Clone, Deserialize, Serialize, ToSchema)]
#[serde(default)]
pub struct SettingsResponse {
    pub max_active_files: u32,
    pub max_chunks_per_file: u32,
    /// Simultaneous connections one host may see across all running transfers; `0` lifts
    /// the limit entirely.
    #[serde(default = "default_connections_per_host")]
    pub max_connections_per_host: u32,
    /// NNTP connections one NZB file may hold at once; `0` is as many as the enabled
    /// servers allow. A value below the servers' total caps the file (RD-108-25).
    pub nntp_connections_per_file: u32,
    pub speed_limit_bytes_per_second: Option<rd_core::ByteCount>,
    pub generate_sha256: bool,
    pub global_proxy_profile_id: Option<rd_core::ProxyProfileId>,
    pub custom_ca_pem: Option<String>,
    pub auto_extract: bool,
    pub archive_max_files: u32,
    pub archive_max_uncompressed_bytes: rd_core::ByteCount,
    pub rar_executable: Option<String>,
    pub rar_tool: String,
    /// Retries per file before a retryable failure becomes final (0–100).
    pub max_retries: u32,
    /// Keep NZB import entries and stored .torrent files after the download finishes;
    /// off removes them automatically on completion.
    #[serde(default = "default_keep_import_history")]
    pub keep_import_history: bool,
    /// Remove finished packages from the queue once they have been finished long enough.
    #[serde(default)]
    pub auto_remove_finished: bool,
    /// How long a package stays after it finished, in hours (1–720).
    #[serde(default = "default_auto_remove_delay_hours")]
    pub auto_remove_delay_hours: u32,
    /// Keep a finished package that still holds a file which did not complete.
    #[serde(default = "default_auto_remove_keep_failed")]
    pub auto_remove_keep_failed: bool,
    /// Remove archive volumes after a successful extraction.
    pub delete_archives_after_extract: bool,
    /// Absolute path of the password list (one per line); empty = `passwords.txt` next to the database.
    pub passwords_file: Option<String>,
    /// Switches the administrator login off (only sensible on a trusted loopback/LAN setup).
    #[serde(default)]
    pub admin_login_disabled: bool,
    /// Address ranges whose `X-Forwarded-For` is believed, as CIDR or bare addresses.
    ///
    /// Empty means no header is read and the peer address is the client, which is the safe
    /// default: without it, anyone could name any client they liked.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
    /// What the outside world calls this service: `https://rd.example.com/downloads`.
    ///
    /// Carries the scheme, host and mount point together so they cannot disagree.
    #[serde(default)]
    pub external_url: Option<String>,
    /// When the session cookie is marked `Secure`.
    #[serde(default)]
    pub cookie_security: rd_authn::CookieSecurity,
    /// Hours without a request after which a sign-in ends (RD-130-09); 1 to 720.
    #[serde(default = "default_session_idle_hours")]
    pub session_idle_hours: u32,
    /// Hours from sign-in after which a session ends however busy it is; 1 to 2160.
    ///
    /// A shorter value ends the sessions already past it at once; a longer one applies from
    /// the next sign-in, because the browser keeps the cookie only as long as it was told.
    #[serde(default = "default_session_max_hours")]
    pub session_max_hours: u32,
    /// Post-processing level for packages without an explicit or category level.
    pub default_level: Option<rd_core::PostprocessLevel>,
    /// Hold new downloads while a package is post-processing.
    pub pause_during_postprocess: bool,
    /// Transfer services switched off entirely. A disabled service refuses new links at
    /// intake and blocks whatever it already had queued, with a reason.
    ///
    /// Every one defaults to on except nothing: switching a service off is an explicit act.
    /// Torrent is the one that also shares data, which is why it has its own switch for that
    /// rather than being covered by this one.
    #[serde(default = "default_true")]
    pub torrent_service_enabled: bool,
    #[serde(default = "default_true")]
    pub usenet_service_enabled: bool,
    #[serde(default = "default_true")]
    pub media_service_enabled: bool,
    #[serde(default = "default_true")]
    pub gallery_service_enabled: bool,
    #[serde(default = "default_true")]
    pub recording_service_enabled: bool,
    #[serde(default = "default_true")]
    pub remote_service_enabled: bool,
    /// Extensions (without dot) deleted from the package folder after unpacking.
    pub cleanup_extensions: Vec<String>,
    /// Delete sample files after unpacking and skip sample archives.
    pub ignore_samples: bool,
    /// Also extract archives found inside extracted archives (depth-capped).
    #[serde(default)]
    pub recursive_unpack: bool,
    /// Verify the CRC32 checksums of any `.sfv` index in the package before unpacking.
    #[serde(default)]
    pub sfv_verify: bool,
    /// Whether a failed PAR2/SFV/RAR verification blocks unpacking and everything after it.
    /// On by default, like SABnzbd's `safe_postproc`; off means the unpack runs anyway and a
    /// broken recovery set beside intact archives no longer locks a package (RD-104-04).
    #[serde(default = "default_true")]
    pub safe_postproc: bool,
    /// Delete the PAR2 recovery set once repair and extraction have both succeeded. Off by
    /// default: it is the only thing that can rescue a damaged package.
    #[serde(default)]
    pub delete_par2: bool,
    /// Download every PAR2 recovery volume of an NZB straight away. Off by default, like
    /// SABnzbd's `enable_all_par`: the main index comes down with the payload, the `vol`
    /// volumes wait, and only a repair that is short of blocks fetches as many of them as the
    /// gap needs (RD-107-04). On restores the older behaviour of fetching all of them.
    #[serde(default)]
    pub enable_all_par: bool,
    /// Post-processing plugin steps enabled by default, by plugin id and in the order they
    /// run. A category may override the list, including with an empty one.
    #[serde(default)]
    pub plugin_steps: Vec<String>,
    /// Whether installed metadata enricher plugins are asked about resolved links
    /// (RD-090-14). Off by default: an enricher reaches a service outside this machine, and
    /// doing that on the strength of having installed a plugin would be a decision nobody
    /// made.
    #[serde(default)]
    pub metadata_enrichment_enabled: bool,
    /// Files containing "sample" in their name count as samples only below this size.
    pub sample_max_bytes: rd_core::ByteCount,
    /// Absolute scripts directory; empty = `scripts` next to the database.
    pub scripts_directory: Option<String>,
    /// Seconds after which a post-processing script is killed (10–86400).
    pub script_timeout_seconds: u32,
    /// Absolute path of yt-dlp; empty = look up on PATH.
    pub media_ytdlp_executable: Option<String>,
    /// Absolute path of ffmpeg; empty = look up on PATH.
    pub media_ffmpeg_executable: Option<String>,
    /// Variant preselected for new media links (`best`, `1080p`, `720p`, `audio_mp3`, …).
    pub media_default_variant: String,
    /// Full default selection for new media links; `None` uses `media_default_variant`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_default_criteria: Option<rd_core::MediaFormatCriteria>,
    /// Default output template for media downloads; `None` keeps the plain file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_output_template: Option<String>,
    /// Hosts handled by the media provider (without `www.`).
    pub media_hosts: Vec<String>,
    /// Concurrent media downloads (1–8).
    pub media_max_parallel: u32,
    /// Timeout of one metadata probe in seconds (5–600).
    pub media_check_timeout_seconds: u32,
    /// Absolute path of gallery-dl; empty = look up in the vendor folders and on PATH.
    #[serde(default)]
    pub gallery_executable: Option<String>,
    /// Hosts handled by the gallery provider (without `www.`).
    #[serde(default = "rd_core::GallerySettings::default_hosts")]
    pub gallery_hosts: Vec<String>,
    /// Concurrent gallery downloads (1–8).
    #[serde(default = "default_gallery_max_parallel")]
    pub gallery_max_parallel: u32,
    /// Concurrent FTP/SFTP transfers (1–8).
    #[serde(default = "default_remote_max_parallel")]
    pub remote_max_parallel: u32,
    /// Connect, login and per-read timeout for FTP/SFTP in seconds (5–600).
    #[serde(default = "default_remote_timeout")]
    pub remote_timeout_seconds: u32,
    /// Whether an unknown SSH host key may be trusted on first use without asking.
    #[serde(default)]
    pub remote_ssh_auto_trust: bool,
    /// Absolute path of streamlink; empty = vendor folders (incl. `vendor/streamlink/bin`)
    /// and PATH.
    #[serde(default)]
    pub record_streamlink_executable: Option<String>,
    /// Stream selection used when a channel has none (`best`, `1080p`, …).
    #[serde(default = "default_record_quality")]
    pub record_default_quality: String,
    /// Seconds between liveness probes of enabled channels (60–3600).
    #[serde(default = "default_record_poll_interval")]
    pub record_poll_interval_seconds: u32,
    /// Concurrent recordings (1–8); recordings never block regular downloads.
    #[serde(default = "default_record_max_parallel")]
    pub record_max_parallel: u32,
    /// Incoming BitTorrent peer port; empty = a random port. Applied on the next start.
    #[serde(default)]
    pub torrent_listen_port: Option<u16>,
    /// Seed until uploaded/downloaded reaches this ratio (0 disables the ratio stop).
    #[serde(default = "default_torrent_seed_ratio")]
    pub torrent_seed_ratio: f64,
    /// Stop seeding after this many minutes; empty = no time limit.
    #[serde(default)]
    pub torrent_seed_time_minutes: Option<u32>,
    /// Seed finished torrents; off completes them immediately after the download.
    #[serde(default = "default_torrent_seeding_enabled")]
    pub torrent_seeding_enabled: bool,
    /// Upload data to peers at all. **Off by default**, so a fresh installation shares
    /// nothing. Distinct from seeding, which only covers the phase after a download
    /// finishes: the engine uploads while downloading too, and this is what stops it.
    #[serde(default)]
    pub torrent_sharing_enabled: bool,
    /// Global torrent upload limit in bytes per second; empty = unlimited. Next start.
    #[serde(default)]
    pub torrent_upload_limit_bytes_per_second: Option<rd_core::ByteCount>,
    /// Network interface every torrent socket binds to; `None` binds to all.
    #[serde(default)]
    pub torrent_bind_interface: Option<String>,
    /// Pause torrent traffic when the bound interface disappears.
    #[serde(default)]
    pub torrent_kill_switch_enabled: bool,
    /// HTTP(S) URL of an IP blocklist the torrent engine loads at startup.
    #[serde(default)]
    pub torrent_ip_blocklist_url: Option<String>,
    #[serde(default)]
    pub torrent_listen_mode: rd_core::TorrentListenMode,
    #[serde(default)]
    pub torrent_peer_limit: Option<u32>,
    #[serde(default)]
    pub torrent_download_limit_bytes_per_second: Option<rd_core::ByteCount>,
    /// SOCKS5 proxy profile for outgoing torrent peer connections.
    #[serde(default)]
    pub torrent_proxy_profile_id: Option<rd_core::ProxyProfileId>,
    /// Ask the router to forward the listen port via UPnP.
    #[serde(default)]
    pub torrent_upnp_enabled: bool,
    /// Port announced to trackers when a mapping uses a different external port.
    #[serde(default)]
    pub torrent_announce_port: Option<u16>,
    /// Show full peer addresses in the torrent peer list instead of the network prefix.
    #[serde(default)]
    pub torrent_peer_addresses_visible: bool,
    /// Weekly windows during which resource-intensive work waits.
    #[serde(default)]
    pub quiet_hours: rd_limits::QuietHours,
    /// Hold back PAR2 repair, unpacking and uploads during quiet hours.
    #[serde(default = "default_true")]
    pub quiet_hours_defer_postprocess: bool,
    /// Group notification deliveries until the quiet period ends.
    #[serde(default = "default_true")]
    pub quiet_hours_defer_notifications: bool,
    /// What runs once the queue and post-processing have drained.
    #[serde(default)]
    pub completion_action: rd_power::CompletionAction,
    /// Script name inside the post-processing scripts directory.
    #[serde(default)]
    pub completion_script: Option<String>,
    /// Seconds a power action counts down before it runs (10–3600).
    #[serde(default = "default_completion_countdown")]
    pub completion_countdown_seconds: u32,
    /// Local approval for standby and shutdown; without it they are never executed.
    #[serde(default)]
    pub power_actions_allowed: bool,
    /// Hold the queue while the machine runs on battery.
    #[serde(default)]
    pub pause_on_battery: bool,
    /// Hold the queue while the connection reports itself as metered.
    #[serde(default)]
    pub pause_on_metered: bool,
    /// Keep the machine awake while downloads or post-processing are actually running.
    #[serde(default)]
    pub prevent_standby: bool,
    /// Keep the display awake too, for a machine somebody watches.
    #[serde(default)]
    pub prevent_display_standby: bool,
    /// Treat links in a package that point at the same file as alternatives, downloading one.
    #[serde(default = "default_mirror_detection")]
    pub mirror_detection: bool,
    /// Run the reconnect script when free downloads are stuck behind an IP limit.
    #[serde(default)]
    pub reconnect_enabled: bool,
    /// Script name inside the post-processing scripts directory.
    #[serde(default)]
    pub reconnect_script: Option<String>,
    /// Weekly windows a reconnect may run in; empty means any time.
    ///
    /// The element type is shared with quiet hours, which is the same shape — a weekday set
    /// and a span of local minutes — and lets the interface reuse the same editor.
    #[serde(default)]
    pub reconnect_windows: Vec<rd_limits::QuietWindow>,
    /// Shortest gap between two reconnects, in minutes (1–1440).
    #[serde(default = "default_reconnect_interval_minutes")]
    pub reconnect_min_interval_minutes: u32,
    /// How long a reconnect may take before it is given up on, in seconds (30–900).
    #[serde(default = "default_reconnect_timeout_seconds")]
    pub reconnect_timeout_seconds: u32,
    /// Allow a reconnect while transfers are running; they are paused and resumed around it.
    #[serde(default)]
    pub reconnect_abort_active: bool,
    /// Addresses asked what the public address is; empty uses the built-in list.
    #[serde(default)]
    pub reconnect_ip_check_urls: Vec<String>,
    /// IANA timezone the bandwidth schedule and its budget periods are read in.
    #[serde(default = "default_bandwidth_timezone")]
    pub bandwidth_timezone: String,
    /// Bandwidth profile applied outside every schedule window; empty = no limits.
    #[serde(default)]
    pub bandwidth_default_profile_id: Option<rd_core::BandwidthProfileId>,
    /// Free space that must remain on a storage root without an own threshold.
    #[serde(default = "default_storage_minimum_free_bytes")]
    pub storage_minimum_free_bytes: rd_core::ByteCount,
    /// Release a blocked storage root by itself once space is free again.
    #[serde(default = "default_storage_auto_resume")]
    pub storage_auto_resume: bool,
    /// A transfer of unknown size may start while `threshold × factor` bytes are free (1–64).
    #[serde(default = "default_storage_unknown_size_headroom")]
    pub storage_unknown_size_headroom: u32,
    /// Absolute directory searched for yt-dlp/ffmpeg/ffprobe/unrar/7z before `PATH`; empty =
    /// the built-in `vendor` folders next to the executable and in the data directory.
    pub vendor_directory: Option<String>,
    /// Whether this installation may download, verify and activate tool versions itself
    /// (RD-102-02). Off by default: fetching executables is not something to start unasked.
    #[serde(default)]
    pub managed_tools_enabled: bool,
    /// `https://` URL of the signed tool manifest; empty = only the manifest compiled into
    /// this build, which is the offline-safe default.
    #[serde(default)]
    pub managed_tools_manifest_url: Option<String>,
    /// Tools whose compatibility verdict is reported but not enforced (RD-102-03). The
    /// warning stays; only the block is dropped, and every skipped block is logged.
    #[serde(default)]
    pub tool_compatibility_overrides: Vec<String>,
    /// Upload finished packages to an rclone remote as the last post-processing step.
    #[serde(default)]
    pub upload_enabled: bool,
    /// rclone target in `remote:path` form; the package folder is created below it.
    #[serde(default)]
    pub upload_remote: Option<String>,
    /// `copy` keeps the local files, `move` removes them after a successful upload.
    #[serde(default = "default_upload_mode")]
    pub upload_mode: String,
    /// Absolute path of rclone; empty = look up in the vendor folders and on PATH.
    #[serde(default)]
    pub rclone_executable: Option<String>,
    /// Absolute path of the domain blocklist (one host per line); empty =
    /// `excluded_domains.txt` next to the database.
    pub excluded_domains_file: Option<String>,
    /// Import `.dlc` containers. Off by default on purpose: the format cannot be decrypted
    /// locally, so every import sends the container's key to the service below.
    #[serde(default)]
    pub dlc_service_enabled: bool,
    /// `dlcrypt` service that unwraps the container key; empty = the JDownloader service.
    #[serde(default)]
    pub dlc_service_endpoint: Option<String>,
    /// Show the cover images an indexer announces for its hits (RD-101-17).
    ///
    /// On by default, unlike the enricher switch next to it, because nothing is fetched on
    /// the strength of it: the addresses arrive with the search answer the subscription
    /// already makes, and only the browser loads the pictures, lazily. It is still a switch
    /// because loading them tells the indexer which hits are on somebody's screen, and the
    /// details themselves stay visible when it is off.
    #[serde(default = "default_true")]
    pub subscription_item_images_enabled: bool,
    /// Port the web UI listens on; applied on the next start. `--listen`/`RDOWNLOADER_LISTEN`
    /// override it.
    pub ui_port: Option<u16>,
    /// How byte counts are rendered in the interface: `binary` (KiB/MiB/GiB, 1024) or
    /// `decimal` (kB/MB/GB, 1000). Purely a display choice — nothing computes with it.
    #[serde(default = "default_byte_display")]
    pub byte_display: String,
    /// Which magnitude of the ladder byte counts are printed in: `auto` picks the step that
    /// fits each value, while `byte`, `kilo`, `mega`, `giga`, `tera` or `peta` pin every value
    /// to that step so a list of sizes can be compared column by column (RD-106-14).
    ///
    /// Named by magnitude rather than by unit because the unit names belong to `byte_display`:
    /// `mega` reads as MiB on the binary ladder and MB on the decimal one.
    #[serde(default = "default_byte_unit")]
    pub byte_unit: String,
    /// Whether the browser tab reports what is running — the queue rate and the number of
    /// active transfers — instead of the application name alone (RD-106-07).
    ///
    /// On by default: a tab that says nothing is what every earlier version had, and the point
    /// of the feature is not having to bring the window forward. It is a switch because a title
    /// that keeps changing is a distraction for some people, and that is not arguable.
    #[serde(default = "default_true")]
    pub title_status_enabled: bool,
    /// Plugin ids the user switched off. They stay installed and listed — otherwise they could
    /// not be switched back on — but are not loaded, compiled or executed.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
    /// Days the transfer statistics keep hourly buckets before folding them into daily ones
    /// (RD-110-01); 1 to the retention.
    #[serde(default = "default_stats_hourly_days")]
    pub stats_hourly_days: u32,
    /// Days the transfer statistics are kept at all (RD-110-01); 7 to 3650. The all-time
    /// totals behind the metrics counters are never thinned.
    #[serde(default = "default_stats_retention_days")]
    pub stats_retention_days: u32,
    /// Log records the structured log store keeps at most (1000-500000); the oldest go
    /// first (RD-110-02).
    #[serde(default = "default_log_retention_records")]
    pub log_retention_records: u32,
    /// Days a log record is kept at most (1-365), whatever the count.
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// Audit records the append-only audit log keeps at most (10000-2000000); the oldest
    /// whole records go first (RD-110-03).
    #[serde(default = "default_audit_retention_records")]
    pub audit_retention_records: u32,
    /// Days an audit record is kept at most (30-3650), whatever the count.
    #[serde(default = "default_audit_retention_days")]
    pub audit_retention_days: u32,
    /// Whether finished spans are exported over OTLP (RD-110-03). Off by default and off
    /// after an upgrade: exporting traces sends the shape of a person's activity to a third
    /// system, which is a decision somebody takes rather than one they discover.
    #[serde(default)]
    pub otlp_enabled: bool,
    /// The collector's OTLP/HTTP traces endpoint, such as
    /// `http://127.0.0.1:4318/v1/traces`. Empty means unconfigured, which is the same as off.
    #[serde(default)]
    pub otlp_endpoint: String,
    /// How long one export attempt may take before it is abandoned (1-60).
    #[serde(default = "default_otlp_timeout_seconds")]
    pub otlp_timeout_seconds: u32,
    /// Seconds between two reconciliation scans of every watched folder (RD-110-31); 5 to
    /// 3600, one value for all folders, taken over by running watchers without a restart.
    #[serde(default = "default_hotfolder_poll_seconds")]
    pub hotfolder_poll_seconds: u32,
}

fn default_stats_hourly_days() -> u32 {
    crate::stats_handlers::DEFAULT_HOURLY_DAYS
}

fn default_stats_retention_days() -> u32 {
    crate::stats_handlers::DEFAULT_RETENTION_DAYS
}

const fn default_hotfolder_poll_seconds() -> u32 {
    rd_core::DEFAULT_HOTFOLDER_POLL_SECONDS
}

impl SettingsResponse {
    fn validate_media(&mut self, max_path: usize) -> Result<(), crate::ApiError> {
        for (field, value) in [
            ("yt-dlp", &mut self.media_ytdlp_executable),
            ("ffmpeg", &mut self.media_ffmpeg_executable),
            ("gallery-dl", &mut self.gallery_executable),
            ("streamlink", &mut self.record_streamlink_executable),
        ] {
            *value = value
                .take()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            if value.as_ref().is_some_and(|text| {
                text.len() > max_path || !std::path::Path::new(text).is_absolute()
            }) {
                return Err(crate::ApiError::bad_request(
                    "settings.media_tool_path_invalid",
                    format!("{field} must be an absolute path of at most {max_path} characters"),
                )
                .with_param("tool", field)
                .with_param("max", max_path));
            }
        }
        self.media_hosts = self
            .media_hosts
            .iter()
            .map(|host| {
                host.trim()
                    .trim_start_matches("www.")
                    .trim_end_matches('/')
                    .to_ascii_lowercase()
            })
            .filter(|host| !host.is_empty())
            .collect();
        if let Some(bad) = self.media_hosts.iter().find(|host| {
            host.len() > 253
                || host.contains('/')
                || host.contains(':')
                || !host.contains('.')
                || host
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.media_host_invalid",
                format!("Media host '{bad}' must be a bare domain name"),
            )
            .with_param("value", bad));
        }
        self.media_default_variant = self.media_default_variant.trim().to_owned();
        // `custom` accompanies an explicit criteria set; the preset ids stay valid so an
        // existing API caller keeps working unchanged.
        let valid_variant = matches!(
            self.media_default_variant.as_str(),
            "best" | "audio_mp3" | "custom"
        ) || self
            .media_default_variant
            .strip_suffix('p')
            .is_some_and(|digits| digits.parse::<u32>().is_ok());
        if !valid_variant {
            return Err(crate::ApiError::bad_request(
                "settings.media_variant_invalid",
                "Default media variant must be 'best', '<height>p', 'audio_mp3' or 'custom'",
            ));
        }
        self.media_output_template = self
            .media_output_template
            .take()
            .map(|template| template.trim().to_owned())
            .filter(|template| !template.is_empty());
        if let Some(template) = self.media_output_template.as_deref() {
            rd_files::validate(template).map_err(|error| {
                crate::ApiError::bad_request("media.template_invalid", error.to_string())
            })?;
        }
        if let Some(criteria) = self.media_default_criteria.take() {
            self.media_default_criteria = Some(criteria.sanitized().map_err(|error| {
                crate::ApiError::bad_request("media.criteria_invalid", error.to_string())
            })?);
        }
        if !(1..=8).contains(&self.media_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "settings.media_parallel_invalid",
                "Concurrent media downloads must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(5..=600).contains(&self.media_check_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.media_timeout_invalid",
                "Media probe timeout must be between 5 and 600 seconds",
            )
            .with_param("min", 5)
            .with_param("max", 600));
        }
        self.gallery_hosts = self
            .gallery_hosts
            .iter()
            .map(|host| {
                host.trim()
                    .trim_start_matches("www.")
                    .trim_end_matches('/')
                    .to_ascii_lowercase()
            })
            .filter(|host| !host.is_empty())
            .collect();
        if let Some(bad) = self.gallery_hosts.iter().find(|host| {
            host.len() > 253
                || host.contains('/')
                || host.contains(':')
                || !host.contains('.')
                || host
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.media_host_invalid",
                format!("Gallery host '{bad}' must be a bare domain name"),
            )
            .with_param("value", bad));
        }
        if !(1..=8).contains(&self.gallery_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "settings.media_parallel_invalid",
                "Concurrent gallery downloads must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(1..=8).contains(&self.remote_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "remote.parallel_invalid",
                "Concurrent FTP/SFTP transfers must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(5..=600).contains(&self.remote_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "remote.timeout_invalid",
                "The FTP/SFTP timeout must be between 5 and 600 seconds",
            )
            .with_param("min", 5)
            .with_param("max", 600));
        }
        self.record_default_quality = self.record_default_quality.trim().to_owned();
        if self.record_default_quality.is_empty() || self.record_default_quality.len() > 50 {
            return Err(crate::ApiError::bad_request(
                "stream.quality_invalid",
                "Default stream quality must be 1-50 characters",
            ));
        }
        if !(60..=3600).contains(&self.record_poll_interval_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.record_poll_interval_invalid",
                "Channel poll interval must be between 60 and 3600 seconds",
            )
            .with_param("min", 60)
            .with_param("max", 3600));
        }
        if !(1..=8).contains(&self.record_max_parallel) {
            return Err(crate::ApiError::bad_request(
                "settings.media_parallel_invalid",
                "Concurrent recordings must be between 1 and 8",
            )
            .with_param("min", 1)
            .with_param("max", 8));
        }
        if !(0.0..=100.0).contains(&self.torrent_seed_ratio) || !self.torrent_seed_ratio.is_finite()
        {
            return Err(crate::ApiError::bad_request(
                "settings.torrent_seed_ratio_invalid",
                "Seed ratio must be between 0 and 100",
            )
            .with_param("min", 0)
            .with_param("max", 100));
        }
        if self
            .torrent_seed_time_minutes
            .is_some_and(|minutes| minutes == 0 || minutes > 60 * 24 * 365)
        {
            return Err(crate::ApiError::bad_request(
                "settings.torrent_seed_time_invalid",
                "Seed time limit must be between 1 minute and one year",
            ));
        }

        if let Some(url) = self.torrent_ip_blocklist_url.as_deref() {
            // The engine loads this itself; only an http(s) URL can ever work.
            let usable = url::Url::parse(url)
                .is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"));
            if !usable {
                return Err(crate::ApiError::bad_request(
                    "torrent.blocklist_invalid",
                    "The IP blocklist must be an http or https URL",
                ));
            }
        }
        if let Some(interface) = self.torrent_bind_interface.as_deref()
            && !interface.trim().is_empty()
            && !rd_torrent::interfaces()
                .iter()
                .any(|candidate| candidate.name == interface)
        {
            return Err(crate::ApiError::bad_request(
                "torrent.interface_unknown",
                "The selected network interface does not exist",
            )
            .with_param("interface", interface.to_owned()));
        }
        Ok(())
    }
}

/// Redaction-safe metadata for one installed resolver version.
///
/// `name` and `description` are the manifest's own values; the plugin manager overlays the
/// localised ones from `/api/v1/plugins/i18n/{locale}` when they exist.
#[derive(Serialize, ToSchema)]
pub struct InstalledPluginResponse {
    /// Whether this is the version that wins among the installed ones for its id.
    ///
    /// Installing never removes an older version, so two can sit side by side; the highest
    /// SemVer is the one loaded. That rule was never wrong, only invisible — the manager listed
    /// both with nothing to separate them, so the leftover looked like a second, equal plugin.
    ///
    /// Says nothing about whether the plugin is switched on; that is a separate choice and
    /// applies to every version of an id at once.
    #[serde(default)]
    pub active: bool,
    /// How many invocations this plugin id has recorded, capped at what the store keeps.
    ///
    /// Only the number, never an entry: the manager offers its diagnostics accordion when this
    /// is greater than zero and fetches the entries themselves when somebody opens it, which is
    /// what keeps diagnostics nobody looks at free. Without it the accordion was a button with
    /// nothing behind it, and the panel it opened could not say whether the plugin had never
    /// run or run without incident.
    ///
    /// Counted per id, so every installed version of one id reports the same number — the
    /// store records the version in the entry, not in its key.
    #[serde(default)]
    pub execution_count: u32,
    pub id: rd_core::PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub homepage: Option<String>,
    pub support_url: Option<String>,
    pub license: Option<String>,
    /// Provider slug this resolver serves.
    pub provider_slug: String,
    /// What the plugin is: `resolver`, `transfer`, or whatever an unknown package claims.
    pub plugin_type: String,
    /// `rdownloader:plugin` WIT version the package was built against.
    pub api_version: String,
    /// Every grant the manifest asks for, as the plugin manager lists them.
    pub capabilities: Vec<String>,
    pub domains: Vec<String>,
    pub max_concurrent_downloads: u32,
}

impl From<rd_plugin_host::PluginManifest> for InstalledPluginResponse {
    fn from(manifest: rd_plugin_host::PluginManifest) -> Self {
        let domains = manifest.domains().to_vec();
        let capabilities = manifest.capabilities.granted();
        let provider_slug = manifest.message_slug().to_owned();
        Self {
            // Decided by the caller, which sees the whole list; one manifest cannot know
            // whether a higher version of itself is installed alongside it.
            active: false,
            // Likewise: the count comes from the execution store, which a manifest cannot read.
            execution_count: 0,
            id: manifest.id,
            name: manifest.name,
            version: manifest.version,
            description: manifest.metadata.description,
            author: manifest.metadata.author,
            homepage: manifest.metadata.homepage,
            support_url: manifest.metadata.support_url,
            license: manifest.metadata.license,
            provider_slug,
            plugin_type: manifest.plugin_type.as_str().to_owned(),
            api_version: manifest.api_version,
            capabilities,
            domains,
            max_concurrent_downloads: manifest.max_concurrent_downloads,
        }
    }
}

/// What the plugin manager shows: the packages that run, and the packages that do not.
///
/// A refused package is listed rather than dropped. It is an artefact the user installed;
/// silently omitting it explains nothing about why a third-party hoster stopped working.
#[derive(Serialize, ToSchema)]
pub struct PluginInventoryResponse {
    pub installed: Vec<InstalledPluginResponse>,
    pub incompatible: Vec<IncompatiblePluginResponse>,
}

/// An installed package this build refuses to run.
#[derive(Serialize, ToSchema)]
pub struct IncompatiblePluginResponse {
    /// Plugin id, or the directory name when the manifest cannot say.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Stable code the UI translates: `plugin.manifest_outdated`,
    /// `plugin.capability_unknown` or `plugin.manifest_unreadable`.
    pub code: String,
}

impl From<rd_plugin_host::IncompatiblePlugin> for IncompatiblePluginResponse {
    fn from(plugin: rd_plugin_host::IncompatiblePlugin) -> Self {
        Self {
            id: plugin.id,
            name: plugin.name,
            version: plugin.version,
            code: plugin.code,
        }
    }
}

/// One recorded plugin invocation, as the diagnostics card shows it.
#[derive(Serialize, ToSchema)]
pub struct PluginExecutionResponse {
    /// Bare UUID a user can quote in a report; it identifies the entry and nothing else.
    pub correlation_id: String,
    pub plugin_version: String,
    pub plugin_type: String,
    /// Entry point that ran: `resolve`, `check`, `probe`, `run`, …
    pub operation: String,
    /// `ok`, `failed`, `crash`, `timeout`, `fuel`, `memory`, `host_error` or `denied`.
    pub outcome: String,
    /// Stable failure code, when there was one.
    pub error_class: Option<String>,
    /// Redacted message; never a credential, a token or a header value.
    pub message: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub duration_ms: i64,
}

impl From<rd_db::PluginExecution> for PluginExecutionResponse {
    fn from(entry: rd_db::PluginExecution) -> Self {
        Self {
            correlation_id: entry.correlation_id,
            plugin_version: entry.plugin_version,
            plugin_type: entry.plugin_type,
            operation: entry.operation,
            outcome: entry.outcome,
            error_class: entry.error_class,
            message: entry.message,
            started_at: entry.started_at,
            duration_ms: entry.duration_ms,
        }
    }
}

/// One plugin signing key the user confirmed on first use.
#[derive(Serialize, ToSchema)]
pub struct PluginTrustedKeyResponse {
    pub key_id: String,
    /// Hex SHA-256 of the public key, as shown when it was confirmed.
    pub fingerprint: String,
    pub plugin_name: Option<String>,
    pub confirmed_at: String,
}

impl From<rd_db::PluginTrustedKey> for PluginTrustedKeyResponse {
    fn from(key: rd_db::PluginTrustedKey) -> Self {
        Self {
            key_id: key.key_id,
            fingerprint: key.fingerprint,
            plugin_name: key.plugin_name,
            confirmed_at: key.confirmed_at,
        }
    }
}

/// One withdrawn plugin package version.
#[derive(Serialize, ToSchema)]
pub struct PluginDigestRevocationResponse {
    /// The package's content digest, 64 lowercase hex characters.
    pub digest: String,
    /// Which plugin it belonged to, when that was known when it was withdrawn.
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub version: Option<String>,
    pub reason: Option<String>,
    pub revoked_at: String,
}

impl From<rd_db::PluginDigestRevocation> for PluginDigestRevocationResponse {
    fn from(row: rd_db::PluginDigestRevocation) -> Self {
        Self {
            digest: row.digest,
            plugin_id: row.plugin_id,
            plugin_name: row.plugin_name,
            version: row.version,
            reason: row.reason,
            revoked_at: row.revoked_at,
        }
    }
}

/// Whether a provider resolves links for its own domains or other hosters' domains
/// (mirrors `rd_provider_registry::ProviderKind`).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKindResponse {
    Hoster,
    Multihoster,
}

impl From<rd_provider_registry::ProviderKind> for ProviderKindResponse {
    fn from(kind: rd_provider_registry::ProviderKind) -> Self {
        match kind {
            rd_provider_registry::ProviderKind::Hoster => Self::Hoster,
            rd_provider_registry::ProviderKind::Multihoster => Self::Multihoster,
        }
    }
}

/// The shape of the credential(s) a provider account stores (mirrors
/// `rd_provider_registry::CredentialKind`).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialsResponse {
    ApiKey,
    UsernamePassword,
    ApiKeyOrCookies,
    Cookies,
    /// One account, two ways to hold it; the settings form asks which one before it asks for
    /// the credential itself. The choices are in `credential_modes`.
    LoginOrApiKey,
    /// Signed in through a redirect; the settings form offers a sign-in rather than a field.
    ///
    /// Renamed for the same reason the manifest spelling is: the derived name would be
    /// `o_auth`, which is nobody's idea of what this is called.
    #[serde(rename = "oauth")]
    OAuth,
    /// Takes no account; the accounts settings leave such a provider out of the list.
    #[serde(rename = "none")]
    NoneRequired,
}

impl From<rd_provider_registry::CredentialKind> for ProviderCredentialsResponse {
    fn from(kind: rd_provider_registry::CredentialKind) -> Self {
        match kind {
            rd_provider_registry::CredentialKind::ApiKey => Self::ApiKey,
            rd_provider_registry::CredentialKind::UsernamePassword => Self::UsernamePassword,
            rd_provider_registry::CredentialKind::ApiKeyOrCookies => Self::ApiKeyOrCookies,
            rd_provider_registry::CredentialKind::Cookies => Self::Cookies,
            rd_provider_registry::CredentialKind::LoginOrApiKey => Self::LoginOrApiKey,
            rd_provider_registry::CredentialKind::OAuth => Self::OAuth,
            rd_provider_registry::CredentialKind::NoneRequired => Self::NoneRequired,
        }
    }
}

/// One entry of the provider registry, exposed for the accounts settings UI.
#[derive(Serialize, ToSchema)]
pub struct ProviderResponse {
    pub slug: String,
    pub display_name: String,
    pub kind: ProviderKindResponse,
    pub credentials: ProviderCredentialsResponse,
    pub username_required: bool,
    /// The credential modes this provider offers, in the order the form should present them.
    /// Empty for every provider with only one way to hold an account.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_modes: Vec<rd_provider_registry::CredentialMode>,
    /// Whether this provider is compiled in or contributed by an installed plugin.
    pub source: ProviderSourceResponse,
    /// Whether an installed plugin can sign this provider in without a key being typed
    /// (RD-090-13). A run-time fact, not a registry one: it depends on what is installed.
    #[serde(default)]
    pub device_flow: bool,
    /// The plugin behind this provider, and the version of it that is in use.
    ///
    /// Two versions of one plugin can be installed at once and the highest wins. That is well
    /// defined but was invisible: the dropdown said "DDownload" either way, so nobody could tell
    /// which one an account would actually be served by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    /// The host of the provider's `cookie_scope`, when its plugin declares one: the one site
    /// whose session the browser extension can hand over to an account (RD-120-45).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie_scope_host: Option<String>,
}

/// Where a provider row came from (mirrors `rd_provider_registry::ProviderSource`).
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSourceResponse {
    Builtin,
    Plugin,
}

impl From<rd_provider_registry::ProviderSource> for ProviderSourceResponse {
    fn from(source: rd_provider_registry::ProviderSource) -> Self {
        match source {
            rd_provider_registry::ProviderSource::Builtin => Self::Builtin,
            rd_provider_registry::ProviderSource::Plugin => Self::Plugin,
        }
    }
}

impl From<&rd_provider_registry::ProviderSpec> for ProviderResponse {
    fn from(spec: &rd_provider_registry::ProviderSpec) -> Self {
        Self {
            slug: spec.slug.clone(),
            display_name: spec.display_name.clone(),
            kind: spec.kind.into(),
            credentials: spec.credentials.into(),
            username_required: spec.username_required,
            credential_modes: spec.credential_modes(),
            source: spec.source.into(),
            // Filled in by the handler, which knows what is installed; the registry does not.
            device_flow: false,
            plugin_id: spec.plugin_id.clone(),
            plugin_version: spec.plugin_version.clone(),
            cookie_scope_host: spec
                .cookie_scope
                .as_deref()
                .and_then(|scope| url::Url::parse(scope).ok())
                .filter(|scope| scope.scheme() == "https")
                .and_then(|scope| scope.host_str().map(str::to_owned)),
        }
    }
}

impl Default for SettingsResponse {
    fn default() -> Self {
        Self {
            max_active_files: 3,
            max_chunks_per_file: 4,
            log_retention_records: default_log_retention_records(),
            log_retention_days: default_log_retention_days(),
            audit_retention_records: default_audit_retention_records(),
            audit_retention_days: default_audit_retention_days(),
            otlp_enabled: false,
            otlp_endpoint: String::new(),
            otlp_timeout_seconds: default_otlp_timeout_seconds(),
            hotfolder_poll_seconds: default_hotfolder_poll_seconds(),
            max_connections_per_host: default_connections_per_host(),
            nntp_connections_per_file: 0,
            speed_limit_bytes_per_second: None,
            generate_sha256: true,
            global_proxy_profile_id: None,
            custom_ca_pem: None,
            auto_extract: false,
            archive_max_files: 20_000,
            archive_max_uncompressed_bytes: rd_core::ByteCount::new(100 * 1024 * 1024 * 1024)
                .expect("default archive limit fits SQLite"),
            rar_executable: None,
            rar_tool: "unrar".to_owned(),
            max_retries: rd_scheduler::DEFAULT_MAX_RETRIES,
            keep_import_history: default_keep_import_history(),
            auto_remove_finished: false,
            auto_remove_delay_hours: default_auto_remove_delay_hours(),
            auto_remove_keep_failed: default_auto_remove_keep_failed(),
            delete_archives_after_extract: false,
            passwords_file: None,
            admin_login_disabled: false,
            trusted_proxies: Vec::new(),
            external_url: None,
            cookie_security: rd_authn::CookieSecurity::default(),
            session_idle_hours: default_session_idle_hours(),
            session_max_hours: default_session_max_hours(),
            default_level: Some(rd_core::PostprocessLevel::Unpack),
            pause_during_postprocess: true,
            torrent_service_enabled: true,
            usenet_service_enabled: true,
            media_service_enabled: true,
            gallery_service_enabled: true,
            recording_service_enabled: true,
            remote_service_enabled: true,
            cleanup_extensions: rd_core::PostprocessSettings::default_cleanup_extensions(),
            ignore_samples: true,
            recursive_unpack: false,
            sfv_verify: true,
            safe_postproc: true,
            delete_par2: false,
            enable_all_par: false,
            plugin_steps: Vec::new(),
            metadata_enrichment_enabled: false,
            sample_max_bytes: rd_core::ByteCount::new(300 * 1024 * 1024)
                .expect("sample limit fits SQLite"),
            scripts_directory: None,
            script_timeout_seconds: 3600,
            media_ytdlp_executable: None,
            media_ffmpeg_executable: None,
            media_default_variant: "best".to_owned(),
            media_default_criteria: None,
            media_output_template: None,
            media_hosts: rd_core::MediaSettings::default_hosts(),
            media_max_parallel: 2,
            media_check_timeout_seconds: 60,
            gallery_executable: None,
            gallery_hosts: rd_core::GallerySettings::default_hosts(),
            gallery_max_parallel: default_gallery_max_parallel(),
            remote_max_parallel: default_remote_max_parallel(),
            remote_timeout_seconds: default_remote_timeout(),
            remote_ssh_auto_trust: false,
            record_streamlink_executable: None,
            record_default_quality: default_record_quality(),
            record_poll_interval_seconds: default_record_poll_interval(),
            record_max_parallel: default_record_max_parallel(),
            torrent_listen_port: None,
            torrent_seed_ratio: default_torrent_seed_ratio(),
            torrent_seed_time_minutes: None,
            torrent_seeding_enabled: default_torrent_seeding_enabled(),
            torrent_sharing_enabled: false,
            torrent_upload_limit_bytes_per_second: None,
            torrent_bind_interface: None,
            torrent_kill_switch_enabled: false,
            torrent_ip_blocklist_url: None,
            torrent_listen_mode: rd_core::TorrentListenMode::default(),
            torrent_peer_limit: None,
            torrent_download_limit_bytes_per_second: None,
            torrent_proxy_profile_id: None,
            torrent_upnp_enabled: false,
            torrent_announce_port: None,
            torrent_peer_addresses_visible: false,
            quiet_hours: rd_limits::QuietHours::default(),
            quiet_hours_defer_postprocess: true,
            quiet_hours_defer_notifications: true,
            completion_action: rd_power::CompletionAction::None,
            completion_script: None,
            completion_countdown_seconds: default_completion_countdown(),
            power_actions_allowed: false,
            pause_on_battery: false,
            pause_on_metered: false,
            prevent_standby: false,
            prevent_display_standby: false,
            mirror_detection: default_mirror_detection(),
            reconnect_enabled: false,
            reconnect_script: None,
            reconnect_windows: Vec::new(),
            reconnect_min_interval_minutes: default_reconnect_interval_minutes(),
            reconnect_timeout_seconds: default_reconnect_timeout_seconds(),
            reconnect_abort_active: false,
            reconnect_ip_check_urls: Vec::new(),
            bandwidth_timezone: default_bandwidth_timezone(),
            bandwidth_default_profile_id: None,
            storage_minimum_free_bytes: default_storage_minimum_free_bytes(),
            storage_auto_resume: default_storage_auto_resume(),
            storage_unknown_size_headroom: default_storage_unknown_size_headroom(),
            byte_display: default_byte_display(),
            byte_unit: default_byte_unit(),
            title_status_enabled: true,
            disabled_plugins: Vec::new(),
            stats_hourly_days: default_stats_hourly_days(),
            stats_retention_days: default_stats_retention_days(),
            vendor_directory: None,
            managed_tools_enabled: false,
            managed_tools_manifest_url: None,
            tool_compatibility_overrides: Vec::new(),
            upload_enabled: false,
            upload_remote: None,
            upload_mode: default_upload_mode(),
            rclone_executable: None,
            excluded_domains_file: None,
            dlc_service_enabled: false,
            subscription_item_images_enabled: true,
            dlc_service_endpoint: None,
            ui_port: None,
        }
    }
}

fn default_upload_mode() -> String {
    "copy".to_owned()
}

fn default_gallery_max_parallel() -> u32 {
    2
}

const fn default_remote_max_parallel() -> u32 {
    2
}

const fn default_remote_timeout() -> u32 {
    60
}

fn default_record_quality() -> String {
    "best".to_owned()
}

fn default_record_poll_interval() -> u32 {
    120
}

fn default_record_max_parallel() -> u32 {
    2
}

fn default_torrent_seed_ratio() -> f64 {
    1.0
}

const fn default_true() -> bool {
    true
}

const fn default_completion_countdown() -> u32 {
    rd_power::DEFAULT_COMPLETION_COUNTDOWN
}

fn default_bandwidth_timezone() -> String {
    rd_core::DEFAULT_BANDWIDTH_TIMEZONE.to_owned()
}

fn default_storage_minimum_free_bytes() -> rd_core::ByteCount {
    rd_core::StorageSettings::default().storage_minimum_free_bytes
}

fn default_storage_auto_resume() -> bool {
    rd_core::StorageSettings::default().storage_auto_resume
}

fn default_byte_display() -> String {
    // Binary keeps the figures the previous versions showed; switching the default would make
    // every size in every installation change overnight for no reason the user asked for.
    "binary".to_owned()
}

fn default_byte_unit() -> String {
    // Scaling every value on its own is what the interface has always done; pinning a unit is
    // the deliberate choice of somebody who wants a column to compare.
    "auto".to_owned()
}

fn default_storage_unknown_size_headroom() -> u32 {
    rd_core::DEFAULT_UNKNOWN_SIZE_HEADROOM
}

fn default_torrent_seeding_enabled() -> bool {
    false
}

/// On: fetching the same bytes twice is never what somebody wanted, and a mirror that is not
/// needed costs nothing where it waits.
const fn default_mirror_detection() -> bool {
    true
}

/// Ten minutes: a router needs a minute or two to come back, and reconnecting in a tight loop
/// against a hoster that is simply refusing gains nothing.
const fn default_reconnect_interval_minutes() -> u32 {
    10
}

/// Three minutes covers a router reboot; past that something else is wrong.
const fn default_reconnect_timeout_seconds() -> u32 {
    180
}

const fn default_session_idle_hours() -> u32 {
    rd_core::DEFAULT_SESSION_IDLE_HOURS
}

const fn default_session_max_hours() -> u32 {
    rd_core::DEFAULT_SESSION_MAX_HOURS
}

/// A day: long enough to notice a finished package, short enough to keep the queue readable.
const fn default_auto_remove_delay_hours() -> u32 {
    24
}

const fn default_log_retention_records() -> u32 {
    rd_core::DEFAULT_LOG_RETENTION_RECORDS
}

const fn default_log_retention_days() -> u32 {
    rd_core::DEFAULT_LOG_RETENTION_DAYS
}

const fn default_audit_retention_records() -> u32 {
    rd_core::DEFAULT_AUDIT_RETENTION_RECORDS
}

const fn default_audit_retention_days() -> u32 {
    rd_core::DEFAULT_AUDIT_RETENTION_DAYS
}

const fn default_otlp_timeout_seconds() -> u32 {
    rd_core::DEFAULT_OTLP_TIMEOUT_SECONDS
}

/// Errors are worth looking at, so a package holding one is kept until it is dealt with.
const fn default_auto_remove_keep_failed() -> bool {
    true
}

fn default_keep_import_history() -> bool {
    true
}

/// A settings document written before the per-host limit existed gets the default rather
/// than an unbounded zero.
const fn default_connections_per_host() -> u32 {
    rd_http::DEFAULT_CONNECTIONS_PER_HOST as u32
}

/// Normalises an rclone target: trimmed, `remote:path` shaped, no control characters.
pub fn normalize_upload_remote(value: Option<String>) -> Result<Option<String>, crate::ApiError> {
    let Some(trimmed) = value
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let valid = trimmed.len() <= 1024
        && trimmed.contains(':')
        && !trimmed.starts_with(':')
        && !trimmed.chars().any(char::is_control);
    if !valid {
        return Err(crate::ApiError::bad_request(
            "settings.upload_remote_invalid",
            "Upload target must have the rclone form 'remote:path'",
        ));
    }
    Ok(Some(trimmed))
}

impl SettingsResponse {
    /// Normalises the post-processing paths and rejects values outside the supported ranges.
    pub fn validate_postprocess(&mut self) -> Result<(), crate::ApiError> {
        const MAX_PATH_LENGTH: usize = 4096;
        const MIN_AUTO_REMOVE_DELAY_HOURS: u32 = 1;
        // A month. Beyond that the setting is indistinguishable from leaving it switched off.
        const MAX_AUTO_REMOVE_DELAY_HOURS: u32 = 720;
        self.validate_media(MAX_PATH_LENGTH)?;
        self.cleanup_extensions = self
            .cleanup_extensions
            .iter()
            .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect();
        if let Some(bad) = self
            .cleanup_extensions
            .iter()
            .find(|value| value.len() > 10 || !value.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Err(crate::ApiError::bad_request(
                "settings.cleanup_extension_invalid",
                format!("Cleanup extension '{bad}' must be 1-10 alphanumeric characters"),
            )
            .with_param("value", bad));
        }
        if self.sample_max_bytes.get() == 0 {
            return Err(crate::ApiError::bad_request(
                "settings.sample_max_bytes_invalid",
                "Sample size limit must be greater than zero",
            ));
        }
        if !(10..=86_400).contains(&self.script_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.script_timeout_invalid",
                "Script timeout must be between 10 and 86400 seconds",
            )
            .with_param("min", 10)
            .with_param("max", 86_400));
        }
        self.scripts_directory = self
            .scripts_directory
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self.scripts_directory.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.scripts_directory_invalid",
                format!("Scripts directory must be an absolute path of at most {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        if !matches!(self.upload_mode.as_str(), "copy" | "move") {
            return Err(crate::ApiError::bad_request(
                "settings.upload_mode_invalid",
                "Upload mode must be 'copy' or 'move'",
            ));
        }
        self.upload_remote = normalize_upload_remote(self.upload_remote.take())?;
        self.rclone_executable = self
            .rclone_executable
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self.rclone_executable.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.rclone_path_invalid",
                format!("rclone must be an absolute path of at most {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        if self.max_retries > rd_scheduler::MAX_CONFIGURABLE_RETRIES {
            return Err(crate::ApiError::bad_request(
                "settings.max_retries_too_high",
                format!(
                    "Retries per file must not exceed {}",
                    rd_scheduler::MAX_CONFIGURABLE_RETRIES
                ),
            )
            .with_param("max", rd_scheduler::MAX_CONFIGURABLE_RETRIES));
        }
        if !(1..=1440).contains(&self.reconnect_min_interval_minutes) {
            return Err(crate::ApiError::bad_request(
                "settings.reconnect_interval_invalid",
                "The gap between reconnects must be between 1 and 1440 minutes",
            ));
        }
        if !(30..=900).contains(&self.reconnect_timeout_seconds) {
            return Err(crate::ApiError::bad_request(
                "settings.reconnect_timeout_invalid",
                "A reconnect must be given between 30 and 900 seconds",
            ));
        }
        for address in &self.reconnect_ip_check_urls {
            let parsed = url::Url::parse(address.trim());
            if !parsed.is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
                return Err(crate::ApiError::bad_request(
                    "settings.reconnect_url_invalid",
                    "An address check must be an http or https URL",
                )
                .with_param("value", address.clone()));
            }
        }
        // A reconnect without a script would hold the queue and then do nothing.
        if self.reconnect_enabled
            && self
                .reconnect_script
                .as_deref()
                .is_none_or(|name| name.trim().is_empty())
        {
            return Err(crate::ApiError::bad_request(
                "settings.reconnect_script_missing",
                "A reconnect needs the name of the script that reconnects",
            ));
        }
        if !(MIN_AUTO_REMOVE_DELAY_HOURS..=MAX_AUTO_REMOVE_DELAY_HOURS)
            .contains(&self.auto_remove_delay_hours)
        {
            return Err(crate::ApiError::bad_request(
                "settings.auto_remove_delay_invalid",
                format!(
                    "The delay before a finished package is removed must be between {MIN_AUTO_REMOVE_DELAY_HOURS} and {MAX_AUTO_REMOVE_DELAY_HOURS} hours"
                ),
            )
            .with_param("min", MIN_AUTO_REMOVE_DELAY_HOURS)
            .with_param("max", MAX_AUTO_REMOVE_DELAY_HOURS));
        }
        if !(1..=1_000_000).contains(&self.archive_max_files) {
            return Err(crate::ApiError::bad_request(
                "settings.archive_max_files_invalid",
                "Archive file limit must be between 1 and 1,000,000",
            )
            .with_param("min", 1)
            .with_param("max", 1_000_000));
        }
        if self.archive_max_uncompressed_bytes.get() == 0 {
            return Err(crate::ApiError::bad_request(
                "settings.archive_max_bytes_invalid",
                "Archive size limit must be greater than zero",
            ));
        }
        if !matches!(self.rar_tool.as_str(), "unrar" | "7z") {
            return Err(crate::ApiError::bad_request(
                "settings.rar_tool_invalid",
                "RAR tool must be either 'unrar' or '7z'",
            ));
        }
        self.rar_executable = self
            .rar_executable
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self
            .rar_executable
            .as_ref()
            .is_some_and(|value| value.len() > MAX_PATH_LENGTH)
        {
            return Err(crate::ApiError::bad_request(
                "settings.rar_executable_too_long",
                format!("RAR tool path must not exceed {MAX_PATH_LENGTH} characters"),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        if self
            .rar_executable
            .as_ref()
            .is_some_and(|value| !std::path::Path::new(value).is_absolute())
        {
            return Err(crate::ApiError::bad_request(
                "settings.rar_executable_not_absolute",
                "RAR tool must be given as an absolute path",
            ));
        }
        self.passwords_file = self
            .passwords_file
            .take()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if self.passwords_file.as_ref().is_some_and(|value| {
            value.len() > MAX_PATH_LENGTH || !std::path::Path::new(value).is_absolute()
        }) {
            return Err(crate::ApiError::bad_request(
                "settings.passwords_file_invalid",
                format!(
                    "Password list must be an absolute path of at most {MAX_PATH_LENGTH} characters"
                ),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        for (code, field, value) in [
            (
                "settings.vendor_directory_invalid",
                "Vendor directory",
                &mut self.vendor_directory,
            ),
            (
                "settings.excluded_domains_file_invalid",
                "Domain blocklist",
                &mut self.excluded_domains_file,
            ),
        ] {
            *value = value
                .take()
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
            if value.as_ref().is_some_and(|text| {
                text.len() > MAX_PATH_LENGTH || !std::path::Path::new(text).is_absolute()
            }) {
                return Err(crate::ApiError::bad_request(
                    code,
                    format!(
                        "{field} must be an absolute path of at most {MAX_PATH_LENGTH} characters"
                    ),
                )
                .with_param("max", MAX_PATH_LENGTH));
            }
        }
        // A manifest served over plain HTTP is a manifest whoever sits on the path can
        // replace. The signature would still be checked, but refusing here says why rather
        // than failing later with "untrusted key" on a document nobody tampered with.
        self.managed_tools_manifest_url = self
            .managed_tools_manifest_url
            .take()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        if self
            .managed_tools_manifest_url
            .as_ref()
            .is_some_and(|url| url.len() > MAX_PATH_LENGTH || !url.starts_with("https://"))
        {
            return Err(crate::ApiError::bad_request(
                "settings.managed_tools_manifest_url_invalid",
                format!(
                    "The tool manifest URL must be an https address of at most                      {MAX_PATH_LENGTH} characters"
                ),
            )
            .with_param("max", MAX_PATH_LENGTH));
        }
        // An override switches off a block for one named tool. A name nothing gates on would
        // look like it did something and do nothing, so it is refused rather than dropped.
        self.tool_compatibility_overrides = std::mem::take(&mut self.tool_compatibility_overrides)
            .into_iter()
            .map(|tool| tool.trim().to_lowercase())
            .filter(|tool| !tool.is_empty())
            .collect();
        self.tool_compatibility_overrides.sort();
        self.tool_compatibility_overrides.dedup();
        if let Some(unknown) = self
            .tool_compatibility_overrides
            .iter()
            .find(|tool| !rd_tools::compat::RULED_TOOLS.contains(&tool.as_str()))
        {
            return Err(crate::ApiError::bad_request(
                "settings.tool_compatibility_override_invalid",
                format!("{unknown} has no compatibility rule to override"),
            )
            .with_param("tool", unknown));
        }
        // Ports below 1024 need elevated privileges on Unix and would make the service fail
        // to start after a restart, i.e. lock the user out of the very UI they configured.
        if self.ui_port.is_some_and(|port| port < 1024) {
            return Err(crate::ApiError::bad_request(
                "settings.ui_port_invalid",
                "The UI port must be between 1024 and 65535",
            )
            .with_param("min", 1024)
            .with_param("max", 65_535));
        }
        Ok(())
    }
}

/// Free and total capacity of one storage root.
#[derive(Serialize, ToSchema)]
pub struct StorageSpace {
    pub id: rd_core::StorageRootId,
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// `None` when the path is currently unreachable.
    pub free_bytes: Option<rd_core::ByteCount>,
    pub total_bytes: Option<rd_core::ByteCount>,
}

/// Aggregated queue counters shown below the download list.
#[derive(Serialize, ToSchema)]
pub struct DownloadSummaryResponse {
    pub queued: u32,
    pub active: u32,
    pub paused: u32,
    pub blocked: u32,
    pub failed: u32,
    pub completed: u32,
    pub total_bytes: rd_core::ByteCount,
    pub committed_bytes: rd_core::ByteCount,
    /// Everything not yet fetched, including entries that are paused, blocked or already
    /// downloaded and now being verified, repaired, unpacked or seeded.
    ///
    /// Deliberately wider than `transferring_remaining_bytes`: this is "how much is still
    /// outstanding", not "how much is on its way". The remaining time is built from the
    /// narrower figure, and the interface names the difference rather than blurring it.
    pub remaining_bytes: rd_core::ByteCount,
    /// Bytes still to fetch across the entries that are actually going to be fetched at the
    /// current rate — queued, waiting to retry, resolving or downloading.
    ///
    /// `None` while one of those entries has no known size, because the sum would then be a
    /// lower bound rather than the remainder.
    pub transferring_remaining_bytes: Option<rd_core::ByteCount>,
    /// Combined smoothed transfer rate of the whole queue, in bytes per second.
    pub bytes_per_second: u64,
    /// Seconds until the queue is through, at the current rate.
    ///
    /// `None` whenever a number would be invented rather than measured: nothing is moving, or
    /// one of the entries still to be fetched has no known size, which would make the figure a
    /// lower bound presented as an answer.
    pub eta_seconds: Option<u64>,
    pub storage: Vec<StorageSpace>,
}

/// One entry's live rate and the remaining time it implies.
#[derive(Serialize, ToSchema)]
pub struct DownloadRateEntry {
    pub id: rd_core::DownloadId,
    pub bytes_per_second: u64,
    /// `None` when the size is unknown or the entry is not moving.
    pub eta_seconds: Option<u64>,
}

/// Live transfer rates: the queue as a whole, and every entry that is moving.
///
/// Entries at rest are left out — their rate is zero and their remaining time is nothing, and
/// saying so for every finished download in a long list is pure payload.
#[derive(Serialize, ToSchema)]
pub struct DownloadRatesResponse {
    pub bytes_per_second: u64,
    /// Bytes still to fetch across queued, retrying, resolving and downloading entries;
    /// `None` while one of them has no known size.
    pub transferring_remaining_bytes: Option<rd_core::ByteCount>,
    /// Seconds until the queue is through, or `None` when no honest figure exists.
    pub eta_seconds: Option<u64>,
    pub downloads: Vec<DownloadRateEntry>,
}

/// Figures only, for the desktop tray.
///
/// Deliberately not `DownloadSummaryResponse`: that one carries storage entries with their
/// paths, and this is served to the capture agent, whose token is scoped for handing links in.
/// The event stream is filtered to intake for the same reason — the full bus would carry
/// download paths and account names. Counts and byte totals say enough for an icon and a
/// tooltip and say nothing about what is being downloaded.
#[derive(Serialize, ToSchema)]
pub struct CaptureSummaryResponse {
    pub active: u32,
    pub queued: u32,
    pub failed: u32,
    /// Bytes committed across everything not finished, and the total where it is known.
    pub committed_bytes: rd_core::ByteCount,
    pub total_bytes: rd_core::ByteCount,
    /// The queue's smoothed transfer rate, in bytes per second.
    ///
    /// Served here so the tray shows the same figure as the web interface. The agent used to
    /// derive its own from two consecutive reads, unsmoothed, which meant two different
    /// answers to the same question; the service keeps the rate now (RD-104-02).
    pub bytes_per_second: u64,
    /// Seconds until the queue is through at that rate, or `null` when no honest figure
    /// exists: an entry still to be fetched whose size is unknown, a rate of zero, a paused
    /// transfer. The same rule the web interface follows, because it is the same number.
    ///
    /// Taken from the very `queue_rate()` value the rate above comes from (RD-108-01). The
    /// estimate was already being computed there and then dropped, so the tray had the rate
    /// but not the time it implies; a second formula here would have been a second answer to
    /// one question, which is the mistake RD-104-02 exists to have ended.
    pub eta_seconds: Option<u64>,
}

/// Hoster domains an account's provider can download from.
#[derive(Serialize, ToSchema)]
pub struct AccountHostersResponse {
    pub account_id: rd_core::AccountId,
    pub provider: String,
    pub hosters: Vec<String>,
}

/// New name for a package **and** for the folder its files live in (RD-106-13).
#[derive(Deserialize, ToSchema)]
pub struct PackageFolderRequest {
    /// New name (1–200 characters), sanitized into a folder name by the same rules a file
    /// rename uses. A folder of that name that already exists is refused, not avoided.
    pub name: String,
}

/// Category and/or priority change for one package.
#[derive(Deserialize, ToSchema)]
pub struct PackageUpdateRequest {
    pub category_id: Option<rd_core::CategoryId>,
    /// Removes the category (default destination) when `true`.
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
    /// New display name (1–200 characters).
    pub name: Option<String>,
    /// Archive password used for extraction.
    ///
    /// Readable again on the package (RD-104-04); see `DownloadPackage::password`.
    pub password: Option<String>,
    /// Removes the stored archive password when `true`.
    #[serde(default)]
    pub clear_password: bool,
    /// Explicit post-processing level; see `clear_postprocess_level` to inherit again.
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub clear_postprocess_level: bool,
    /// Post-processing script file name inside the scripts directory.
    pub script: Option<String>,
    #[serde(default)]
    pub clear_script: bool,
}

/// Packages to extract manually.
#[derive(Deserialize, ToSchema)]
pub struct PackageExtractRequest {
    pub ids: Vec<rd_core::PackageId>,
}

/// Packages to remove from the download list.
///
/// `force` is the difference between tidying up and throwing work away: without it a package
/// whose files are still running, waiting or seeding is refused, because removing it cancels
/// those files and deletes what they had already written.
#[derive(Deserialize, ToSchema)]
pub struct PackageDeleteRequest {
    pub ids: Vec<rd_core::PackageId>,
    /// Cancels running files and removes the package anyway.
    #[serde(default)]
    pub force: bool,
}

/// Which finished packages the "clear the list" action should remove.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackageClearScope {
    /// Packages in which every file succeeded.
    Completed,
    /// Packages that hold a failed or blocked file and have nothing left to do.
    Failed,
    /// Every package that is not working any more, whatever the outcome.
    All,
}

/// Bulk removal of finished packages.
#[derive(Deserialize, ToSchema)]
pub struct PackageClearRequest {
    pub scope: PackageClearScope,
}

/// A package the clear pass deliberately left alone, and the stable code saying why.
#[derive(Serialize, ToSchema)]
pub struct PackageClearSkip {
    pub package_id: rd_core::PackageId,
    /// The package name, so the reader can find it without looking up the id.
    pub name: String,
    /// `package.members_active`, `package.members_seeding`, `package.postprocess_running`
    /// or `package.members_unfinished`.
    pub code: String,
}

/// What a clear pass did: whole packages removed, and the ones it refused to touch.
#[derive(Serialize, ToSchema)]
pub struct PackageClearResponse {
    pub removed: usize,
    pub skipped: Vec<PackageClearSkip>,
}

/// New file name for a queued, paused or failed download.
#[derive(Deserialize, ToSchema)]
pub struct DownloadRenameRequest {
    pub file_name: String,
}

/// Category and/or priority change for several packages.
#[derive(Deserialize, ToSchema)]
pub struct PackageBulkRequest {
    pub ids: Vec<rd_core::PackageId>,
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub clear_category: bool,
    pub priority: Option<rd_core::DownloadPriority>,
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    #[serde(default)]
    pub clear_postprocess_level: bool,
    pub script: Option<String>,
    #[serde(default)]
    pub clear_script: bool,
}

/// Complete queue order; packages are positioned in the given sequence.
#[derive(Deserialize, ToSchema)]
pub struct PackageReorderRequest {
    pub ids: Vec<rd_core::PackageId>,
}

/// Complete file order of one package; the ids have to be exactly its files, each once.
#[derive(Deserialize, ToSchema)]
pub struct DownloadReorderRequest {
    pub package_id: rd_core::PackageId,
    pub ids: Vec<rd_core::DownloadId>,
}

/// Bulk action applied to individual files.
#[derive(Clone, Copy, Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadBulkAction {
    Pause,
    Resume,
    Cancel,
    Remove,
    /// Discards partial data and checkpoints and queues the file again from zero; a finished
    /// payload is left on disk, so the fresh attempt lands beside it under a free name.
    Reset,
    /// Like `Reset`, and deletes the finished payload as well.
    ResetDeleteFiles,
}

#[derive(Deserialize, ToSchema)]
pub struct DownloadBulkRequest {
    pub ids: Vec<rd_core::DownloadId>,
    pub action: DownloadBulkAction,
}

#[derive(Serialize, ToSchema)]
pub struct DownloadBulkResponse {
    pub affected: u32,
    pub errors: Vec<String>,
}

/// Options for resetting a single file.
#[derive(Deserialize, ToSchema)]
pub struct DownloadResetRequest {
    /// Delete a finished payload as well. Off by default: a reset that keeps the file lets the
    /// fresh attempt land beside it instead of destroying the only copy.
    #[serde(default)]
    pub delete_completed_files: bool,
}

/// Files whose packages should be extracted.
#[derive(Deserialize, ToSchema)]
pub struct DownloadExtractRequest {
    pub ids: Vec<rd_core::DownloadId>,
}

/// Reusable per-domain session or authentication profile. Credential fields are write-only
/// and never appear in a response.
#[derive(Deserialize, ToSchema)]
pub struct CreateAuthProfileRequest {
    pub name: String,
    /// Bare host or full URL; the path selects which profile matches a URL.
    pub scope: String,
    pub include_subdomains: bool,
    pub method: rd_core::AuthMethod,
    /// Username for `basic`.
    pub username: Option<String>,
    /// Netscape `cookies.txt` content or a `Cookie` header for `cookies`; the password for
    /// `basic`; the token for `bearer`.
    #[schema(write_only)]
    pub secret: Option<String>,
    /// Private key and certificate chain as one PEM bundle.
    #[schema(write_only)]
    pub certificate_pem: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub enabled: bool,
}

/// Editable profile fields. Empty credential fields preserve their stored values.
#[derive(Deserialize, ToSchema)]
pub struct UpdateAuthProfileRequest {
    pub name: String,
    pub scope: String,
    pub include_subdomains: bool,
    pub method: rd_core::AuthMethod,
    pub username: Option<String>,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub certificate_pem: Option<String>,
    pub clear_certificate: bool,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub enabled: bool,
}

/// Cookies handed over by a capture client for one approved domain.
#[derive(Deserialize, ToSchema)]
pub struct CaptureCookiesRequest {
    pub name: Option<String>,
    pub scope: String,
    pub include_subdomains: bool,
    #[schema(write_only)]
    pub cookies: String,
}

/// Which profile a single download uses.
#[derive(Deserialize, ToSchema)]
pub struct SetDownloadAuthProfileRequest {
    pub auth_profile: rd_core::AuthProfileSelection,
}

/// Redaction-safe result of a live auth profile check.
#[derive(Serialize, ToSchema)]
pub struct AuthProfileTestResponse {
    pub reachable: bool,
    pub authenticated: bool,
    pub status: Option<u16>,
    pub url: String,
}

/// A new stored login for an FTP, FTPS or SFTP server.
///
/// Credential fields are write-only: they are handed to the secret store on arrival and
/// no endpoint ever returns them or their `vault://` reference.
#[derive(Deserialize, ToSchema)]
pub struct CreateRemoteCredentialRequest {
    pub name: String,
    pub protocol: rd_core::RemoteProtocol,
    /// Bare host name or IP address; a URL is accepted and reduced to its host.
    pub host: String,
    /// `None` uses the protocol's default port.
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth_mode: rd_core::RemoteAuthMode,
    /// FTP data connections use PASV/EPSV; `false` asks for active mode instead.
    #[serde(default = "default_true")]
    pub passive: bool,
    /// Password for `password`, unused for the other modes.
    #[schema(write_only)]
    pub secret: Option<String>,
    /// OpenSSH or PEM private key for `private_key`.
    #[schema(write_only)]
    pub private_key: Option<String>,
    /// Passphrase protecting `private_key`, when it has one.
    #[schema(write_only)]
    pub passphrase: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Editable login fields. Empty credential fields preserve their stored values.
#[derive(Deserialize, ToSchema)]
pub struct UpdateRemoteCredentialRequest {
    pub name: String,
    pub protocol: rd_core::RemoteProtocol,
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub auth_mode: rd_core::RemoteAuthMode,
    #[serde(default = "default_true")]
    pub passive: bool,
    #[schema(write_only)]
    pub secret: Option<String>,
    #[schema(write_only)]
    pub private_key: Option<String>,
    #[schema(write_only)]
    pub passphrase: Option<String>,
    /// Drops the stored private key instead of keeping it.
    #[serde(default)]
    pub clear_private_key: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Redaction-safe result of a live remote login check.
#[derive(Serialize, ToSchema)]
pub struct RemoteCredentialTestResponse {
    pub reachable: bool,
    pub authenticated: bool,
    /// Stable failure code when the check did not succeed.
    pub code: Option<String>,
    /// Parameters belonging to `code`, including an SSH fingerprint awaiting confirmation.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: rd_core::MessageParams,
}

/// Confirms one SSH host key as trusted.
///
/// The fingerprint is required rather than implied: confirming "whatever the server offers
/// next" would make the trust store meaningless, so the caller has to name the key it saw.
#[derive(Deserialize, ToSchema)]
pub struct TrustSshHostKeyRequest {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:<base64>`, exactly as reported by the failure that blocked the transfer.
    pub fingerprint: String,
}

/// The file selection of one remote directory candidate.
#[derive(Deserialize, ToSchema)]
pub struct RemoteListingPlanRequest {
    /// Paths that are excluded; excluding a directory excludes everything below it.
    #[serde(default)]
    pub excluded: Vec<String>,
}
