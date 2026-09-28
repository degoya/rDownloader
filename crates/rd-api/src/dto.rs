use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

mod settings;
mod settings_validation;

pub use settings::*;
pub use settings_validation::*;

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
/// always gets `capture:*`, while an API client picks its areas.
#[derive(Deserialize, ToSchema)]
pub struct ApiTokenRequest {
    pub label: String,
    /// The areas this token may reach, as scope strings.
    ///
    /// Empty mints `api:read`. A caller that names scopes gets exactly those: nothing here
    /// widens a request, because a minting call that quietly grants more than it was asked
    /// for is the one mistake this whole model exists to prevent.
    #[serde(default)]
    pub scopes: Vec<String>,
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
    /// Splitting, remux, sidecars and reconnect delay for this channel (RD-080-09).
    #[serde(default)]
    pub recording: rd_core::RecordingPolicy,
}

fn default_channel_enabled() -> bool {
    true
}

const fn default_true() -> bool {
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

/// Redaction-safe metadata for one installed resolver version.
///
/// `name` and `description` are the manifest's own values; the plugin manager overlays the
/// localised ones from `/api/v1/plugins/i18n/{locale}` when they exist.
#[derive(Serialize, ToSchema)]
pub struct InstalledPluginResponse {
    /// Whether this is the version new work of its id runs on right now.
    ///
    /// Installing never removes an older version, so two can sit side by side; without a
    /// version choice the highest SemVer is the one loaded, and with one (RD-140-02) the chosen
    /// version is. That rule was never wrong, only invisible — the manager listed both with
    /// nothing to separate them, so the leftover looked like a second, equal plugin.
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
    /// One entry per installed plugin id: which version runs, which one is under test, and
    /// how updates arrive (RD-140-02).
    pub lifecycle: Vec<crate::plugin_lifecycle::PluginLifecycleResponse>,
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
    /// Signed in with a code, or holding a pasted API key, chosen per account (RD-150-09); the
    /// choices are in `credential_modes`, and in the `oauth` one the form offers a sign-in
    /// rather than a field.
    #[serde(rename = "oauth_or_api_key")]
    OAuthOrApiKey,
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
            rd_provider_registry::CredentialKind::OAuthOrApiKey => Self::OAuthOrApiKey,
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
    /// The same refusals as `errors`, coded and in the same order, for the interface to
    /// translate; `errors` stays for the clients that read the English text.
    pub refusals: Vec<MessageResponse>,
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
