use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{
    ByteCount, DownloadFile, DownloadId, DownloadPackage, DownloadState, ExpectedChecksum,
    PackageId,
};
use sqlx::FromRow;
use url::Url;

use crate::{json_column::lenient, parse_id};

#[path = "models_queries.rs"]
mod queries;

pub(crate) use queries::{
    committed_bytes_total, downloads_blocked_by, downloads_for_package, downloads_page,
    failed_downloads, get_download, get_download_from_connection, get_package, list_downloads,
    list_packages, load_transfer, packages_page, startable_downloads, transform_checkpoint,
};

/// Fields required to create a package.
#[derive(Clone, Debug)]
pub struct NewPackage {
    pub id: PackageId,
    pub name: String,
    pub destination: String,
    pub category_id: Option<rd_core::CategoryId>,
    pub priority: rd_core::DownloadPriority,
    /// Explicit post-processing level (`None` inherits category/default).
    pub postprocess_level: Option<rd_core::PostprocessLevel>,
    /// Post-processing script name (`None` inherits).
    pub script: Option<String>,
    /// What the enrichers found, written with the row rather than after it.
    ///
    /// It used to arrive as a second write right behind the insert, which meant the package was
    /// readable for the moment in between — with an empty enrichment. That window is what
    /// migration 0063 is named against, and the only way to close it is to write both at once.
    pub enrichment: Vec<rd_core::EnrichmentField>,
}

/// Fields required to enqueue a file.
#[derive(Clone, Debug)]
pub struct NewDownload {
    pub id: DownloadId,
    pub package_id: PackageId,
    pub source: Url,
    pub file_name: String,
    pub total_bytes: Option<ByteCount>,
    pub expected_checksum: Option<ExpectedChecksum>,
    pub account_id: Option<rd_core::AccountId>,
    pub proxy_profile_id: Option<rd_core::ProxyProfileId>,
    /// Which auth profile the job uses (auto-match, none, or a pinned one).
    pub auth_profile: rd_core::AuthProfileSelection,
    /// Only queued and paused are valid initial states.
    pub initial_state: DownloadState,
    /// Transport of the file (`Http` unless a runner kind is chosen).
    pub kind: rd_core::DownloadKind,
    /// Variant selection for media files.
    pub media: Option<rd_core::MediaSelection>,
    /// Stored login for FTP/SFTP files; `None` matches by host and port at transfer time.
    pub remote_credential_id: Option<rd_core::RemoteCredentialId>,
    /// Consented replay template written in the same transaction as the download row.
    pub replay: Option<Box<NewReplayTemplate>>,
    /// The vaulted link fragment this download inherits from its candidate (RD-110-38).
    pub secret_fragment: Option<Box<NewSecretFragment>>,
    /// Key shared by links that point at the same file; `None` means no mirror group.
    pub mirror_group: Option<String>,
    /// What the enrichers found for this file; see [`NewPackage::enrichment`].
    pub enrichment: Vec<rd_core::EnrichmentField>,
}

/// Replay template of a new download, written atomically with it.
#[derive(Clone, Debug)]
pub struct NewReplayTemplate {
    pub request: rd_core::CapturedRequest,
    pub consent: rd_core::ReplayConsent,
    pub body_ref: Option<String>,
    pub candidate_id: Option<rd_core::CandidateId>,
}

/// A vaulted link fragment moving from a candidate to the download it became (RD-110-38).
///
/// Ownership *moves*: the candidate's column is cleared in the same transaction that writes
/// the download row, so exactly one row points at the vault entry and deleting either end
/// can never leave a secret behind or take one that is still in use.
#[derive(Clone, Debug)]
pub struct NewSecretFragment {
    /// The `vault://` reference; never the fragment itself.
    pub reference: String,
    /// Candidate the reference is taken from.
    pub candidate_id: Option<rd_core::CandidateId>,
}

/// Persisted byte range and its last post-sync checkpoint.
#[derive(Clone, Debug)]
pub struct PersistedChunk {
    pub id: rd_core::ChunkId,
    pub start: u64,
    pub end: Option<u64>,
    pub committed: u64,
}

/// Validator and range metadata required for crash-safe resume.
#[derive(Clone, Debug)]
pub struct TransferMetadata {
    pub total_bytes: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub chunks: Vec<PersistedChunk>,
}

#[derive(FromRow)]
pub(crate) struct PackageRow {
    id: String,
    name: String,
    state: String,
    destination: String,
    created_at: DateTime<Utc>,
    category_id: Option<String>,
    priority: i64,
    position: i64,
    has_password: i64,
    kind: String,
    nzb_import_id: Option<String>,
    postprocess_level: Option<String>,
    script: Option<String>,
    postprocess_stage: Option<String>,
    postprocess_percent: Option<i64>,
    postprocess_current: Option<String>,
    extraction_result: Option<String>,
    completed_at: Option<DateTime<Utc>>,
    enrichment_json: Option<String>,
}

#[derive(FromRow)]
struct DownloadRow {
    id: String,
    package_id: String,
    source_url: String,
    file_name: String,
    state: String,
    total_bytes: Option<i64>,
    committed_bytes: i64,
    retry_count: i64,
    next_retry_at: Option<DateTime<Utc>>,
    checksum_algorithm: Option<String>,
    checksum_value: Option<String>,
    computed_checksum_algorithm: Option<String>,
    computed_checksum_value: Option<String>,
    last_error_json: Option<String>,
    account_id: Option<String>,
    proxy_profile_id: Option<String>,
    auth_profile_id: Option<String>,
    auth_profile_pinned: bool,
    recording_json: Option<String>,
    position: i64,
    kind: String,
    nzb_file_id: Option<String>,
    media_json: Option<String>,
    remote_credential_id: Option<String>,
    mirror_group: Option<String>,
    recovery: bool,
    enrichment_json: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Reads the `kind` column back, through the same serde representation that wrote it.
///
/// This used to be a hand-written match, which meant a new variant was written correctly and
/// read back as `Http` until somebody noticed — a silent downgrade of every row of that kind.
/// Deriving both directions from the same source removes that failure mode; the fallback now
/// only fires for a value no version of this application ever wrote.
pub(crate) fn parse_kind(value: &str) -> rd_core::DownloadKind {
    serde_json::from_str(&format!("\"{value}\"")).unwrap_or_else(|_| {
        tracing::warn!(kind = value, "unknown download kind, treating as http");
        rd_core::DownloadKind::Http
    })
}

/// The `postprocess_level` column of `table`; an unknown level is logged and reads as none.
pub(crate) fn parse_level(
    value: Option<&str>,
    table: &str,
    row: impl std::fmt::Display,
) -> Option<rd_core::PostprocessLevel> {
    value.and_then(|text| {
        lenient(
            serde_json::from_str(&format!("\"{text}\"")),
            table,
            "postprocess_level",
            row,
        )
    })
}

/// Reads an enrichment column back (RD-107-02).
///
/// A column that cannot be parsed reads as "no fields" rather than failing the row: these are
/// additions beside the core data, and a package that refuses to load because a plugin's
/// field list is malformed would be a far worse outcome than a missing chip.
pub(crate) fn parse_enrichment(
    value: Option<&str>,
    table: &str,
    row: impl std::fmt::Display,
) -> Vec<rd_core::EnrichmentField> {
    value
        .and_then(|text| lenient(serde_json::from_str(text), table, "enrichment_json", row))
        .unwrap_or_default()
}

impl TryFrom<PackageRow> for DownloadPackage {
    type Error = anyhow::Error;

    fn try_from(row: PackageRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            state: row.state.parse().context("parse package state")?,
            created_at: row.created_at,
            destination: row.destination,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
            priority: rd_core::DownloadPriority::from_i32(
                i32::try_from(row.priority).unwrap_or_default(),
            ),
            position: row.position,
            has_password: row.has_password != 0,
            // In the vault (RD-190-04); `Database::reveal_archive_passwords` fills it for the
            // answers that show it.
            password: None,
            kind: parse_kind(&row.kind),
            nzb_import_id: row.nzb_import_id.as_deref().map(parse_id).transpose()?,
            completed_at: row.completed_at,
            postprocess_level: parse_level(row.postprocess_level.as_deref(), "packages", &row.id),
            script: row.script,
            postprocess: rd_core::PostprocessStatus {
                stage: row
                    .postprocess_stage
                    .as_deref()
                    .and_then(|stage| stage.parse().ok()),
                percent: row
                    .postprocess_percent
                    .and_then(|value| u8::try_from(value).ok()),
                current: row.postprocess_current,
            },
            extraction_result: row
                .extraction_result
                .as_deref()
                .and_then(|value| value.parse().ok()),
            enrichment: parse_enrichment(row.enrichment_json.as_deref(), "packages", &row.id),
        })
    }
}

impl TryFrom<DownloadRow> for DownloadFile {
    type Error = anyhow::Error;

    fn try_from(row: DownloadRow) -> Result<Self> {
        let expected_checksum = match (row.checksum_algorithm, row.checksum_value) {
            (Some(algorithm), Some(value)) => Some(ExpectedChecksum {
                algorithm: serde_json::from_str(&format!("\"{algorithm}\""))?,
                value,
            }),
            (None, None) => None,
            _ => bail!("incomplete checksum metadata"),
        };
        let computed_checksum = match (row.computed_checksum_algorithm, row.computed_checksum_value)
        {
            (Some(algorithm), Some(value)) => Some(ExpectedChecksum {
                algorithm: serde_json::from_str(&format!("\"{algorithm}\""))?,
                value,
            }),
            (None, None) => None,
            _ => bail!("incomplete computed checksum metadata"),
        };

        Ok(Self {
            id: parse_id(&row.id)?,
            package_id: parse_id(&row.package_id)?,
            source: Url::parse(&row.source_url)?,
            file_name: row.file_name,
            state: row.state.parse().context("parse download state")?,
            total_bytes: row.total_bytes.map(i64_to_bytes).transpose()?,
            committed_bytes: i64_to_bytes(row.committed_bytes)?,
            retry_count: u32::try_from(row.retry_count).context("invalid retry count")?,
            next_retry_at: row.next_retry_at,
            expected_checksum,
            computed_checksum,
            last_error: row
                .last_error_json
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            account_id: row.account_id.as_deref().map(parse_id).transpose()?,
            proxy_profile_id: row.proxy_profile_id.as_deref().map(parse_id).transpose()?,
            remote_credential_id: row
                .remote_credential_id
                .as_deref()
                .map(parse_id)
                .transpose()?,
            mirror_group: row.mirror_group,
            recovery: row.recovery,
            recording: row.recording_json.as_deref().and_then(|value| {
                lenient(
                    serde_json::from_str(value),
                    "downloads",
                    "recording_json",
                    &row.id,
                )
            }),
            auth_profile: rd_core::AuthProfileSelection::from_columns(
                row.auth_profile_id.as_deref().map(parse_id).transpose()?,
                row.auth_profile_pinned,
            ),
            position: row.position,
            kind: parse_kind(&row.kind),
            nzb_file_id: row.nzb_file_id.as_deref().map(parse_id).transpose()?,
            media: row
                .media_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .context("parse media selection")?,
            enrichment: parse_enrichment(row.enrichment_json.as_deref(), "downloads", &row.id),
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

/// Queue order: priority first, then manual position, then age.
pub(crate) const PACKAGE_ORDER: &str =
    "ORDER BY packages.priority DESC, packages.position ASC, packages.created_at ASC";

pub(crate) const PACKAGE_COLUMNS: &str = "SELECT packages.id, packages.name, packages.state, packages.destination, packages.created_at, \
     packages.category_id, packages.priority, packages.position, \
     packages.password_ref IS NOT NULL AS has_password, packages.kind, \
     packages.nzb_import_id, \
     packages.postprocess_level, packages.script, packages.postprocess_stage, \
     packages.postprocess_percent, packages.postprocess_current, packages.extraction_result, \
     packages.completed_at, packages.enrichment_json \
     FROM packages";

fn i64_to_bytes(value: i64) -> Result<ByteCount> {
    let value = u64::try_from(value).context("negative byte count")?;
    ByteCount::new(value).map_err(anyhow::Error::msg)
}
