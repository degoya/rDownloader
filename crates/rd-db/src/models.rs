use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{
    ByteCount, DownloadFile, DownloadId, DownloadPackage, DownloadState, ExpectedChecksum,
    PackageId,
};
use sqlx::{FromRow, SqliteConnection, SqlitePool};
use url::Url;

use crate::{error::StoreError, parse_id};

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
    password: Option<String>,
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

pub(crate) fn parse_level(value: Option<&str>) -> Option<rd_core::PostprocessLevel> {
    value.and_then(|text| serde_json::from_str(&format!("\"{text}\"")).ok())
}

/// Reads an enrichment column back (RD-107-02).
///
/// A column that cannot be parsed reads as "no fields" rather than failing the row: these are
/// additions beside the core data, and a package that refuses to load because a plugin's
/// field list is malformed would be a far worse outcome than a missing chip.
pub(crate) fn parse_enrichment(value: Option<&str>) -> Vec<rd_core::EnrichmentField> {
    value
        .and_then(|text| serde_json::from_str(text).ok())
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
            password: row.password,
            kind: parse_kind(&row.kind),
            nzb_import_id: row.nzb_import_id.as_deref().map(parse_id).transpose()?,
            completed_at: row.completed_at,
            postprocess_level: parse_level(row.postprocess_level.as_deref()),
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
            enrichment: parse_enrichment(row.enrichment_json.as_deref()),
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
            recording: row
                .recording_json
                .as_deref()
                .and_then(|value| serde_json::from_str(value).ok()),
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
            enrichment: parse_enrichment(row.enrichment_json.as_deref()),
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
     packages.password IS NOT NULL AS has_password, packages.password, packages.kind, \
     packages.nzb_import_id, \
     packages.postprocess_level, packages.script, packages.postprocess_stage, \
     packages.postprocess_percent, packages.postprocess_current, packages.extraction_result, \
     packages.completed_at, packages.enrichment_json \
     FROM packages";

pub(crate) async fn list_packages(pool: &SqlitePool) -> Result<Vec<DownloadPackage>> {
    sqlx::query_as::<_, PackageRow>(&format!("{PACKAGE_COLUMNS} {PACKAGE_ORDER}"))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

pub(crate) async fn list_downloads(pool: &SqlitePool) -> Result<Vec<DownloadFile>> {
    sqlx::query_as::<_, DownloadRow>(&format!(
        "{DOWNLOAD_COLUMNS} {PACKAGE_ORDER}, downloads.position ASC, downloads.created_at ASC"
    ))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Returns one package's files in queue order.
///
/// Reading the whole `downloads` table and filtering in Rust pulled every JSON blob column
/// of every download ever made through the process once per completed file; the
/// `downloads_package_idx` index exists so this does not happen.
pub(crate) async fn downloads_for_package(
    pool: &SqlitePool,
    package_id: PackageId,
) -> Result<Vec<DownloadFile>> {
    sqlx::query_as::<_, DownloadRow>(&format!(
        "{DOWNLOAD_COLUMNS} WHERE downloads.package_id = ? \
         ORDER BY downloads.position ASC, downloads.created_at ASC"
    ))
    .bind(package_id.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Ids of the blocked downloads whose recorded cause is `reason`.
///
/// Rows blocked before `block_reason` existed hold NULL and are deliberately not matched by
/// any reason: guessing a cause for them would be the very mistake the column was added to
/// stop, so they wait for a manual resume.
pub(crate) async fn downloads_blocked_by(
    pool: &SqlitePool,
    reason: &str,
) -> Result<Vec<DownloadId>> {
    sqlx::query_scalar::<_, String>(
        "SELECT id FROM downloads WHERE state = 'blocked' AND block_reason = ?",
    )
    .bind(reason)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|id| parse_id::<DownloadId>(&id))
    .collect()
}

pub(crate) async fn get_download(
    pool: &SqlitePool,
    id: DownloadId,
) -> Result<Option<DownloadFile>> {
    sqlx::query_as::<_, DownloadRow>(GET_DOWNLOAD)
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

pub(crate) async fn get_download_from_connection(
    connection: &mut SqliteConnection,
    id: DownloadId,
) -> Result<Option<DownloadFile>> {
    sqlx::query_as::<_, DownloadRow>(GET_DOWNLOAD)
        .bind(id.to_string())
        .fetch_optional(connection)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

const DOWNLOAD_COLUMNS: &str = "SELECT downloads.id, downloads.package_id, downloads.source_url, downloads.file_name, downloads.state, downloads.total_bytes, downloads.committed_bytes, downloads.retry_count, downloads.next_retry_at, \
     downloads.checksum_algorithm, downloads.checksum_value, downloads.computed_checksum_algorithm, downloads.computed_checksum_value, downloads.last_error_json, downloads.account_id, downloads.proxy_profile_id, downloads.auth_profile_id, downloads.auth_profile_pinned, downloads.position, downloads.kind, downloads.nzb_file_id, downloads.media_json, downloads.recording_json, downloads.remote_credential_id, downloads.mirror_group, downloads.recovery, downloads.enrichment_json, downloads.created_at, downloads.updated_at \
     FROM downloads JOIN packages ON packages.id = downloads.package_id";

const GET_DOWNLOAD: &str = "SELECT id, package_id, source_url, file_name, state, total_bytes, committed_bytes, retry_count, next_retry_at, \
     checksum_algorithm, checksum_value, computed_checksum_algorithm, computed_checksum_value, last_error_json, account_id, proxy_profile_id, auth_profile_id, auth_profile_pinned, position, kind, nzb_file_id, media_json, recording_json, remote_credential_id, mirror_group, recovery, enrichment_json, created_at, updated_at FROM downloads WHERE id = ?";

/// The finished provider-chunk MACs of one download, and which description wrote them.
///
/// A stored MAC that is not sixteen bytes, or an index past what fits in memory, is a row no
/// resume may build on, so it ends the read rather than being skipped: the condensed value
/// needs every chunk MAC of one stream, and a set with a hole in it verifies nothing.
pub(crate) async fn transform_checkpoint(
    connection: &mut sqlx::SqliteConnection,
    id: DownloadId,
) -> Result<(Option<String>, Vec<(usize, [u8; 16])>)> {
    let rows: Vec<(i64, Vec<u8>, String)> = sqlx::query_as(
        "SELECT chunk_index, mac, fingerprint FROM transform_chunk_macs \
         WHERE download_id = ? ORDER BY chunk_index",
    )
    .bind(id.to_string())
    .fetch_all(&mut *connection)
    .await?;
    let mut fingerprint = None;
    let mut macs = Vec::with_capacity(rows.len());
    for (index, mac, written_by) in rows {
        let mac: [u8; 16] = mac
            .try_into()
            .map_err(|_| anyhow::anyhow!("stored chunk MAC is not 16 bytes"))?;
        let index = usize::try_from(index).context("chunk index out of range")?;
        fingerprint.get_or_insert(written_by);
        macs.push((index, mac));
    }
    Ok((fingerprint, macs))
}

pub(crate) async fn load_transfer(pool: &SqlitePool, id: DownloadId) -> Result<TransferMetadata> {
    let row = sqlx::query_as::<_, TransferRow>(
        "SELECT total_bytes, etag, last_modified FROM downloads WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .context(StoreError::not_found("download not found"))?;
    let chunks = sqlx::query_as::<_, ChunkRow>(
        "SELECT id, start_offset, end_offset, committed_offset FROM chunks \
         WHERE download_id = ? ORDER BY start_offset",
    )
    .bind(id.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect::<Result<Vec<_>>>()?;
    Ok(TransferMetadata {
        total_bytes: row.total_bytes.map(i64_to_u64).transpose()?,
        etag: row.etag,
        last_modified: row.last_modified,
        chunks,
    })
}

#[derive(FromRow)]
struct TransferRow {
    total_bytes: Option<i64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(FromRow)]
struct ChunkRow {
    id: String,
    start_offset: i64,
    end_offset: Option<i64>,
    committed_offset: i64,
}

impl TryFrom<ChunkRow> for PersistedChunk {
    type Error = anyhow::Error;

    fn try_from(row: ChunkRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            start: i64_to_u64(row.start_offset)?,
            end: row.end_offset.map(i64_to_u64).transpose()?,
            committed: i64_to_u64(row.committed_offset)?,
        })
    }
}

fn i64_to_u64(value: i64) -> Result<u64> {
    u64::try_from(value).context("negative persisted size")
}

fn i64_to_bytes(value: i64) -> Result<ByteCount> {
    let value = u64::try_from(value).context("negative byte count")?;
    ByteCount::new(value).map_err(anyhow::Error::msg)
}
