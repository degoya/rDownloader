//! What the restore routes take and answer (RD-160-03).
//!
//! The passphrase travels in a request body only, `write_only` in the contract, and is never
//! part of an answer, a log line or an audit record.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Which archive to open: exactly one of the three.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct RestoreSourceRequest {
    /// An archive uploaded through `/api/v1/backups/restore/uploads`.
    #[serde(default)]
    pub upload_id: Option<String>,
    /// An absolute path to an archive on this machine (a mounted NAS included).
    #[serde(default)]
    pub path: Option<String>,
    /// A successful run of the history, read from its local folder.
    #[serde(default)]
    pub run_id: Option<String>,
}

/// A storage root of the backup, moved to a folder on this machine.
#[derive(Clone, Debug, Deserialize, ToSchema)]
pub struct RestoreMappingRequest {
    pub storage_root_id: String,
    /// Absolute on this machine.
    pub path: String,
}

/// A preview: the archive and the passphrase, nothing else.
#[derive(Deserialize, ToSchema)]
pub struct RestorePreviewRequest {
    pub source: RestoreSourceRequest,
    #[schema(write_only)]
    pub passphrase: String,
}

/// A test restore or a restore: the archive, the passphrase and the storage roots to move.
#[derive(Deserialize, ToSchema)]
pub struct RestoreRequest {
    pub source: RestoreSourceRequest,
    #[schema(write_only)]
    pub passphrase: String,
    #[serde(default)]
    pub mappings: Vec<RestoreMappingRequest>,
}

/// An upload in progress.
#[derive(Serialize, ToSchema)]
pub struct RestoreUploadResponse {
    pub id: String,
    /// Bytes received so far; the next chunk starts here.
    pub size: u64,
    /// The largest chunk one request may carry.
    pub chunk_limit: u64,
}

/// Where the next chunk of an upload starts.
#[derive(Deserialize, IntoParams)]
pub struct RestoreUploadChunkQuery {
    pub offset: u64,
}

/// One kind of member of the archive.
#[derive(Serialize, ToSchema)]
pub struct RestorePartGroupResponse {
    /// `settings`, `database`, `plugin_trust`, `partial_transfers`, `torrent_session` or
    /// `torrent_file`.
    pub kind: String,
    pub count: usize,
    pub size: u64,
}

/// A storage root the backup names.
#[derive(Serialize, ToSchema)]
pub struct RestoreRootResponse {
    pub id: String,
    pub name: String,
    pub path: String,
    /// Whether the path is an absolute path on this machine; a path from another system
    /// wants a mapping.
    pub native: bool,
}

/// Another stored path the backup refers to.
#[derive(Serialize, ToSchema)]
pub struct RestorePathResponse {
    /// `hotfolder` or `partial_transfer`.
    pub kind: String,
    pub path: String,
    pub native: bool,
}

/// What a preview found, from the manifest and the small parts only.
#[derive(Serialize, ToSchema)]
pub struct RestorePreviewResponse {
    pub archive_name: String,
    pub archive_size: u64,
    pub format_version: u32,
    /// The version of rDownloader that wrote the backup.
    pub app_version: String,
    /// The version running here.
    pub current_version: String,
    /// Whether a newer version wrote the backup; its database may not open here.
    pub from_newer_version: bool,
    pub created_at: DateTime<Utc>,
    pub parts: Vec<RestorePartGroupResponse>,
    pub storage_roots: Vec<RestoreRootResponse>,
    /// Hot folders and unfinished transfers' folders, the first fifty.
    pub paths: Vec<RestorePathResponse>,
    pub categories: usize,
    pub accounts: usize,
    pub proxy_profiles: usize,
    pub usenet_servers: usize,
    pub subscriptions: usize,
    pub hotfolders: usize,
    /// Whether the settings carry their credentials, sealed.
    pub credentials_included: bool,
    pub plugin_trust_rows: usize,
    pub partial_transfers: usize,
}

/// A finding of a test restore or a restore.
#[derive(Clone, Serialize, ToSchema)]
pub struct RestoreProblemResponse {
    /// `error` refuses the restore; `warning` is something to do after it.
    pub severity: String,
    /// A stable code, translated like an error code.
    pub code: String,
    pub count: usize,
    /// The first few paths or rows it concerns.
    pub examples: Vec<String>,
}

/// Which migrations wrote the database copy.
#[derive(Serialize, ToSchema)]
pub struct RestoreSchemaResponse {
    pub applied: Option<i64>,
    pub known: i64,
    /// Migrations the test restore applied to the copy.
    pub migrated: usize,
}

/// How much the database copy holds.
#[derive(Serialize, ToSchema)]
pub struct RestoreCountsResponse {
    pub packages: u64,
    pub downloads: u64,
    pub unfinished: u64,
    pub torrents: u64,
    pub storage_roots: u64,
    pub categories: u64,
    pub accounts: u64,
    pub hotfolders: u64,
}

/// A storage root and where the restore puts it.
#[derive(Serialize, ToSchema)]
pub struct RestoreRootPlanResponse {
    pub id: String,
    pub path: String,
    pub native: bool,
    pub mapped_to: Option<String>,
    /// Whether the folder the root ends up on exists here now.
    pub exists_here: bool,
}

/// What a test restore found; the same report a restore answers with.
#[derive(Serialize, ToSchema)]
pub struct RestoreReportResponse {
    /// No finding of severity `error`.
    pub ok: bool,
    pub schema: RestoreSchemaResponse,
    pub counts: RestoreCountsResponse,
    pub roots: Vec<RestoreRootPlanResponse>,
    /// Stored paths moved with their root.
    pub moved_paths: usize,
    pub restored_credentials: usize,
    pub problems: Vec<RestoreProblemResponse>,
}

/// Where a restore stands.
#[derive(Serialize, ToSchema)]
pub struct RestoreStatusResponse {
    /// `none`, `staged` (the next start switches), `switching` (a start is switching now) or
    /// `failed` (the restored state did not start; the previous one runs).
    pub state: String,
    pub archive_name: Option<String>,
    pub staged_at: Option<DateTime<Utc>>,
    pub backup_created_at: Option<DateTime<Utc>>,
    pub app_version: Option<String>,
    pub failed_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
}

/// A staged restore, and the report of the checks it passed.
#[derive(Serialize, ToSchema)]
pub struct RestoreStagedResponse {
    pub status: RestoreStatusResponse,
    pub report: RestoreReportResponse,
}
