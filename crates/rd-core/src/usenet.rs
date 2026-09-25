/// Provider of a link that is an NZB document rather than a file to download.
///
/// Assigned from the address at intake and, for an indexer link that hides its extension,
/// from the content type during the online check (RD-080-11). Such a candidate is *imported*
/// into the Usenet queue rather than saved to disk — downloading the NZB itself and leaving
/// it in the download folder is exactly the wrong outcome.
pub const NZB_PROVIDER: &str = "nzb";

/// Content types an NZB document is served with.
pub const NZB_CONTENT_TYPES: &[&str] = &["application/x-nzb", "application/nzb", "text/x-nzb"];

/// The provider a declared media type names, if it names one of ours.
///
/// A feed states what its enclosure is; an indexer's download address is an API call with no
/// telling extension, so this is the only thing that identifies it short of fetching it. The
/// online check derives the same answer from a response header, but only when the server
/// answers a HEAD with a type at all — which is exactly what an indexer that rate-limits
/// unauthenticated calls does not do.
#[must_use]
pub fn provider_for_media_type(media_type: &str) -> Option<&'static str> {
    let essence = media_type
        .split(';')
        .next()
        .unwrap_or(media_type)
        .trim()
        .to_ascii_lowercase();
    if NZB_CONTENT_TYPES.contains(&essence.as_str()) {
        return Some(NZB_PROVIDER);
    }
    if crate::TORRENT_CONTENT_TYPES.contains(&essence.as_str()) {
        return Some(crate::TORRENT_PROVIDER);
    }
    None
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ByteCount, CategoryId, ImportMode, NzbFileId, NzbImportId, NzbSegmentId, ProxyProfileId,
    UsenetServerId,
};

/// Redaction-safe NNTP endpoint configuration.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct UsenetServer {
    pub id: UsenetServerId,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub username: Option<String>,
    pub has_password: bool,
    pub proxy_profile_id: Option<ProxyProfileId>,
    pub priority: i32,
    pub max_connections: u16,
    pub enabled: bool,
}

/// Persistent lifecycle of an imported NZB.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NzbImportState {
    /// Parsed and waiting for review in the LinkGrabber.
    Imported,
    /// Handed over to the download queue as a package (progress lives there).
    Enqueued,
    /// Parsing or enqueueing failed.
    Failed,
}

/// Crash-recoverable state of one NNTP article.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NzbSegmentState {
    Queued,
    Downloading,
    Completed,
    Failed,
}

/// Persistent kind of one postprocessing operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PostprocessKind {
    Par2,
    /// CRC32 verification of the files an `.sfv` index lists.
    Sfv,
    /// Integrity test of a RAR set (`unrar t` / `7z t`), used as the substitute check when
    /// neither PAR2 nor an `.sfv` index answered whether the payload arrived intact
    /// (RD-104-04, SABnzbd's `try_rar_check`).
    RarTest,
    ExtractZip,
    ExtractSevenZip,
    ExtractRar,
    /// Removal of archive volumes after a successful extraction.
    DeleteArchives,
    /// Removal of the PAR2 recovery set once it is no longer needed — after unpacking has
    /// succeeded, never straight after the repair, because a repair that is followed by a
    /// failed unpack still has to be repeatable.
    DeletePar2,
    /// Deletion of unwanted files (cleanup list, samples) after unpacking.
    Cleanup,
    /// Joining a livestream recording's segments into one container with ffmpeg
    /// (RD-080-09). Persistent, so a crash mid-remux resumes rather than losing the step.
    Remux,
    /// A step contributed by an installed post-processing plugin. `source` is the plugin id,
    /// which is what makes one row distinguishable from another when several are enabled.
    PluginStep,
    /// User post-processing script.
    Script,
    /// rclone upload of the package folder to a configured remote.
    Upload,
}

/// Crash-recoverable lifecycle of a postprocessing operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PostprocessState {
    Queued,
    Running,
    Completed,
    Skipped,
    Failed,
}

/// Persistent, redaction-safe postprocessing checkpoint.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct PostprocessStep {
    /// Owning NZB import or download package id (UUID text).
    pub owner_id: String,
    pub kind: PostprocessKind,
    pub source_path: String,
    pub state: PostprocessState,
    pub output_path: Option<String>,
    pub message: Option<String>,
    /// Stable code for the outcome this step recorded, translated by the interface.
    ///
    /// `message` stays the English text the server produced and remains the fallback for a
    /// code nobody knows; the code is what makes an outcome like "the recovery set is too
    /// small" sayable in four languages instead of only in the server's own words
    /// (RD-107-04). Set on the outcomes that are worth naming, `None` on the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Flat parameters interpolated into the translated `code` (counts, names).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: crate::MessageParams,
    pub updated_at: DateTime<Utc>,
    /// Pipeline order (par2 → unpack → delete → cleanup → script).
    #[serde(default)]
    pub position: i64,
    /// 0–100 while running, when the step reports progress.
    #[serde(default)]
    pub progress_percent: Option<u8>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// What a plugin step wrote when it last stopped, handed back to it verbatim.
    ///
    /// Never serialised: it is a plugin's own bookkeeping, meaningless to anything else, and
    /// an API that returned it would be publishing the internals of somebody else's code.
    #[serde(skip)]
    #[schema(ignore)]
    pub checkpoint: Option<Vec<u8>>,
}

/// Persisted article metadata and its latest verified CRC.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NzbSegmentStatus {
    pub id: NzbSegmentId,
    pub number: u32,
    pub bytes: ByteCount,
    pub message_id: String,
    pub state: NzbSegmentState,
    pub server_attempts: u32,
    pub crc32: Option<String>,
    pub part_begin: Option<ByteCount>,
    pub part_end: Option<ByteCount>,
}

/// One NZB file with all persistent segment states.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NzbFileStatus {
    pub id: NzbFileId,
    pub import_id: NzbImportId,
    pub subject: String,
    pub poster: String,
    pub groups: Vec<String>,
    pub total_bytes: ByteCount,
    pub ordinal: u32,
    pub output_path: Option<String>,
    pub assembly_name: Option<String>,
    pub declared_size: Option<ByteCount>,
    pub segments: Vec<NzbSegmentStatus>,
}

/// Summary stored after a bounded, entity-free NZB parse.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NzbImport {
    pub id: NzbImportId,
    pub name: String,
    pub sha256: String,
    pub state: NzbImportState,
    pub file_count: u32,
    pub segment_count: u32,
    pub total_bytes: ByteCount,
    pub category_id: Option<CategoryId>,
    /// Queue priority chosen at import time; `None` = the default at enqueue time.
    #[serde(default)]
    pub priority: Option<crate::DownloadPriority>,
    pub import_mode: ImportMode,
    pub source_path: Option<String>,
    pub error: Option<String>,
    pub duplicate: bool,
    /// Whether an archive password (from `{{password}}` in the file name) is stored.
    #[serde(default)]
    pub has_password: bool,
    /// The stored archive password, in clear; see `DownloadPackage::password` (RD-104-04).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Place in the LinkGrabber's manual order, shared with `collector_packages`.
    ///
    /// Without it the list cannot render the order it just saved: the two kinds interleave, so a
    /// client that only sees creation times has no way to put an import back where it was
    /// dropped. Deliberately not `#[serde(default)]`: this only ever comes out of the database,
    /// where the column is `NOT NULL`, and a default would make it optional in the generated
    /// contract — leaving every client to invent a fallback position for a value that is always
    /// there.
    pub position: i64,
    pub created_at: DateTime<Utc>,
}
