//! The package and download reads: queue-ordered lists, single rows and the resume metadata.

use anyhow::{Context, Result};
use rd_core::{DownloadFile, DownloadId, DownloadPackage, DownloadState, PackageId};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use super::{
    DownloadRow, PACKAGE_COLUMNS, PACKAGE_ORDER, PackageRow, PersistedChunk, TransferMetadata,
};
use crate::{error::StoreError, page_binds, parse_id};

/// The id after the queue order, so packages equal in everything else still have one order and a
/// page cut from it in SQL is the same slice every time (RD-191-05).
const PACKAGE_ID_LAST: &str = "packages.id ASC";

pub(crate) async fn list_packages(pool: &SqlitePool) -> Result<Vec<DownloadPackage>> {
    sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(format!(
        "{PACKAGE_COLUMNS} {PACKAGE_ORDER}, {PACKAGE_ID_LAST}"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// One page of [`list_packages`] and the length of the whole list, read in one transaction
/// like [`downloads_page`] (RD-191-05).
pub(crate) async fn packages_page(
    pool: &SqlitePool,
    offset: u64,
    limit: Option<u64>,
) -> Result<(Vec<DownloadPackage>, u64)> {
    let (limit, offset) = page_binds(offset, limit);
    let mut transaction = pool.begin().await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM packages")
        .fetch_one(&mut *transaction)
        .await?;
    let rows = sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(format!(
        "{PACKAGE_COLUMNS} {PACKAGE_ORDER}, {PACKAGE_ID_LAST} LIMIT ? OFFSET ?"
    )))
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let packages = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>>>()?;
    Ok((packages, u64::try_from(total).unwrap_or_default()))
}

/// One package by id, read the way [`list_packages`] reads every one.
pub(crate) async fn get_package(
    pool: &SqlitePool,
    id: rd_core::PackageId,
) -> Result<Option<DownloadPackage>> {
    sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(format!(
        "{PACKAGE_COLUMNS} WHERE packages.id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

/// The order of a file among the others once its package's place is settled. The id comes
/// last so that rows equal in everything else still have one order, and a page cut from it in
/// SQL is the same slice every time (RD-1120-17).
const DOWNLOAD_ORDER: &str = "downloads.position ASC, downloads.created_at ASC, downloads.id ASC";

pub(crate) async fn list_downloads(pool: &SqlitePool) -> Result<Vec<DownloadFile>> {
    sqlx::query_as::<_, DownloadRow>(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} {PACKAGE_ORDER}, {DOWNLOAD_ORDER}"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// One page of [`list_downloads`] and the length of the whole list, read in one transaction so
/// the count and the page describe the same moment (RD-1120-17).
///
/// `LIMIT`/`OFFSET` rather than a key set: the REST route's `offset` already means "rows to
/// skip", the order runs over five columns of two tables, and a key-set cursor would be a new
/// parameter the clients do not send. `limit: None` is every row after the offset
/// ([`page_binds`]).
pub(crate) async fn downloads_page(
    pool: &SqlitePool,
    offset: u64,
    limit: Option<u64>,
) -> Result<(Vec<DownloadFile>, u64)> {
    let (limit, offset) = page_binds(offset, limit);
    let mut transaction = pool.begin().await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM downloads JOIN packages ON packages.id = downloads.package_id",
    )
    .fetch_one(&mut *transaction)
    .await?;
    let rows = sqlx::query_as::<_, DownloadRow>(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} {PACKAGE_ORDER}, {DOWNLOAD_ORDER} LIMIT ? OFFSET ?"
    )))
    .bind(limit)
    .bind(offset)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let downloads = rows
        .into_iter()
        .map(TryInto::try_into)
        .collect::<Result<Vec<_>>>()?;
    Ok((downloads, u64::try_from(total).unwrap_or_default()))
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
    sqlx::query_as::<_, DownloadRow>(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} WHERE downloads.package_id = ? \
         ORDER BY downloads.position ASC, downloads.created_at ASC"
    )))
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

/// The rows the dispatcher may start, `queued` and `retry_wait`, in queue order.
///
/// Through `downloads_state_idx`: the dispatcher asks twice a second, and an idle queue of
/// finished downloads answered with the whole table, JSON blobs included (audit 1.9.1, TR-08).
/// Whether a retry is due is the caller's to judge against its own clock.
pub(crate) async fn startable_downloads(pool: &SqlitePool) -> Result<Vec<DownloadFile>> {
    sqlx::query_as::<_, DownloadRow>(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} WHERE downloads.state IN (?, ?) \
         {PACKAGE_ORDER}, downloads.position ASC, downloads.created_at ASC"
    )))
    .bind(DownloadState::Queued.to_string())
    .bind(DownloadState::RetryWait.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// The `failed` rows in queue order, for the automatic retry (RD-191-12); through the same
/// state index as [`startable_downloads`].
pub(crate) async fn failed_downloads(pool: &SqlitePool) -> Result<Vec<DownloadFile>> {
    sqlx::query_as::<_, DownloadRow>(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} WHERE downloads.state = ? \
         {PACKAGE_ORDER}, downloads.position ASC, downloads.created_at ASC"
    )))
    .bind(DownloadState::Failed.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// The committed bytes of every download together, summed by SQLite.
pub(crate) async fn committed_bytes_total(pool: &SqlitePool) -> Result<u64> {
    let total =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(committed_bytes), 0) FROM downloads")
            .fetch_one(pool)
            .await?;
    Ok(u64::try_from(total).unwrap_or(0))
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

/// The columns a `DownloadRow` reads, once for both queries below (DB-12). Qualified, so the
/// list also reads right beside the join; SQLite names each result column without the table.
macro_rules! download_columns {
    () => {
        "downloads.id, downloads.package_id, downloads.source_url, downloads.file_name, downloads.state, downloads.total_bytes, downloads.committed_bytes, downloads.retry_count, downloads.next_retry_at, \
         downloads.checksum_algorithm, downloads.checksum_value, downloads.computed_checksum_algorithm, downloads.computed_checksum_value, downloads.last_error_json, downloads.account_id, downloads.proxy_profile_id, downloads.auth_profile_id, downloads.auth_profile_pinned, downloads.position, downloads.kind, downloads.nzb_file_id, downloads.media_json, downloads.recording_json, downloads.remote_credential_id, downloads.mirror_group, downloads.recovery, downloads.enrichment_json, downloads.created_at, downloads.updated_at"
    };
}

const DOWNLOAD_COLUMNS: &str = concat!(
    "SELECT ",
    download_columns!(),
    " FROM downloads JOIN packages ON packages.id = downloads.package_id"
);

const GET_DOWNLOAD: &str = concat!(
    "SELECT ",
    download_columns!(),
    " FROM downloads WHERE id = ?"
);

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
