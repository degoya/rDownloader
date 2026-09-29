//! Source sets of queued downloads, their health and the chunk marks built on them
//! (RD-150-03).
//!
//! The reading half and the SQL the writer runs. What a source *is* — the order, the checks,
//! the backoff — is `rd_core::source_set`; what lives here is how it is written down so that
//! a restart finds the same order, the same failures and the same isolated mirrors.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{
    CandidateId, ChecksumAlgorithm, ChunkId, DownloadId, DownloadSource, PieceHashes,
    SourceOutcome, SourceProtocol, SourceSet, source_backoff,
};
use sqlx::{Row, SqliteConnection, SqlitePool};

/// What the chunk engine recorded about one chunk beyond its offsets.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkMark {
    pub chunk_id: ChunkId,
    /// The source that delivered the chunk's most recent bytes, when one is known.
    pub source_position: Option<u32>,
    /// Whether the chunk's pieces were checked against their hashes.
    pub verified: bool,
}

/// Writes a download's sources and piece hashes, inside the transaction creating the row.
pub(crate) async fn insert_set(
    connection: &mut SqliteConnection,
    download_id: DownloadId,
    set: &SourceSet,
    now: DateTime<Utc>,
) -> Result<()> {
    for (position, source) in set.sources.iter().enumerate() {
        let protocol = SourceProtocol::from_scheme(source.url.scheme())
            .context("a checked source set carries only known schemes")?;
        sqlx::query(
            "INSERT INTO download_sources (download_id, position, url, protocol, priority, \
             location, local_network, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(download_id.to_string())
        .bind(i64::try_from(position)?)
        .bind(source.url.as_str())
        .bind(protocol.as_str())
        .bind(source.priority.map(i64::from))
        .bind(&source.location)
        .bind(set.local_network)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    }
    if let Some(pieces) = &set.pieces {
        sqlx::query(
            "INSERT INTO download_piece_hashes (download_id, algorithm, piece_length, \
             hashes_json) VALUES (?, ?, ?, ?)",
        )
        .bind(download_id.to_string())
        .bind(algorithm_name(pieces.algorithm)?)
        .bind(i64::try_from(pieces.length)?)
        .bind(serde_json::to_string(&pieces.hashes)?)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

fn algorithm_name(algorithm: ChecksumAlgorithm) -> Result<String> {
    Ok(serde_json::to_string(&algorithm)?
        .trim_matches('"')
        .to_owned())
}

/// Every source of a download, in their fixed order. Empty for a download without a set.
pub(crate) async fn list_sources(
    readers: &SqlitePool,
    download_id: DownloadId,
) -> Result<Vec<DownloadSource>> {
    let rows = sqlx::query(
        "SELECT position, url, protocol, priority, location, failures, backoff_until, \
         isolated_code, last_error_code, delivered_bytes, local_network FROM download_sources \
         WHERE download_id = ? ORDER BY position",
    )
    .bind(download_id.to_string())
    .fetch_all(readers)
    .await?;
    rows.iter()
        .map(|row| {
            let protocol: String = row.get("protocol");
            Ok(DownloadSource {
                position: u32::try_from(row.get::<i64, _>("position"))?,
                url: url::Url::parse(&row.get::<String, _>("url"))?,
                protocol: SourceProtocol::from_scheme(&protocol)
                    .context("unknown source protocol")?,
                priority: row
                    .get::<Option<i64>, _>("priority")
                    .and_then(|value| u32::try_from(value).ok()),
                location: row.get("location"),
                failures: u32::try_from(row.get::<i64, _>("failures")).unwrap_or(u32::MAX),
                backoff_until: row.get("backoff_until"),
                isolated_code: row.get("isolated_code"),
                last_error_code: row.get("last_error_code"),
                delivered_bytes: u64::try_from(row.get::<i64, _>("delivered_bytes"))
                    .unwrap_or_default(),
                local_network: row.get::<bool, _>("local_network"),
            })
        })
        .collect()
}

/// The piece hashes of a download, when its set stated them.
pub(crate) async fn piece_hashes(
    readers: &SqlitePool,
    download_id: DownloadId,
) -> Result<Option<PieceHashes>> {
    let Some(row) = sqlx::query(
        "SELECT algorithm, piece_length, hashes_json FROM download_piece_hashes \
         WHERE download_id = ?",
    )
    .bind(download_id.to_string())
    .fetch_optional(readers)
    .await?
    else {
        return Ok(None);
    };
    let algorithm: ChecksumAlgorithm =
        serde_json::from_str(&format!("\"{}\"", row.get::<String, _>("algorithm")))?;
    Ok(Some(PieceHashes {
        algorithm,
        length: u64::try_from(row.get::<i64, _>("piece_length"))?,
        hashes: serde_json::from_str(&row.get::<String, _>("hashes_json"))?,
    }))
}

/// Records what an attempt learned about one source.
///
/// A delivery clears the failure count and the backoff; a failure counts up and moves the
/// backoff out ([`source_backoff`]); an isolation is final and is never undone by a later
/// delivery, because a source that sent wrong bytes once is not trusted with the next chunk.
pub(crate) async fn record_outcome(
    connection: &mut SqliteConnection,
    download_id: DownloadId,
    position: u32,
    outcome: &SourceOutcome,
    now: DateTime<Utc>,
) -> Result<()> {
    let id = download_id.to_string();
    let position = i64::from(position);
    let result = match outcome {
        SourceOutcome::Delivered { bytes } => {
            sqlx::query(
                "UPDATE download_sources SET failures = 0, backoff_until = NULL, \
                 delivered_bytes = delivered_bytes + ?, updated_at = ? \
                 WHERE download_id = ? AND position = ?",
            )
            .bind(i64::try_from(*bytes).unwrap_or(i64::MAX))
            .bind(now)
            .bind(&id)
            .bind(position)
            .execute(&mut *connection)
            .await?
        }
        SourceOutcome::Failed {
            code,
            retry_after_seconds,
        } => {
            let failures: i64 = sqlx::query_scalar(
                "SELECT failures FROM download_sources WHERE download_id = ? AND position = ?",
            )
            .bind(&id)
            .bind(position)
            .fetch_optional(&mut *connection)
            .await?
            .context("download source not found")?;
            let failures = u32::try_from(failures)
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            let until = now + source_backoff(failures, *retry_after_seconds);
            sqlx::query(
                "UPDATE download_sources SET failures = ?, backoff_until = ?, \
                 last_error_code = ?, updated_at = ? WHERE download_id = ? AND position = ?",
            )
            .bind(i64::from(failures))
            .bind(until)
            .bind(code)
            .bind(now)
            .bind(&id)
            .bind(position)
            .execute(&mut *connection)
            .await?
        }
        SourceOutcome::Isolated { code } => {
            sqlx::query(
                "UPDATE download_sources SET isolated_code = COALESCE(isolated_code, ?), \
                 last_error_code = ?, updated_at = ? WHERE download_id = ? AND position = ?",
            )
            .bind(code)
            .bind(code)
            .bind(now)
            .bind(&id)
            .bind(position)
            .execute(&mut *connection)
            .await?
        }
    };
    if result.rows_affected() == 0 {
        bail!(crate::StoreError::not_found("download source not found"));
    }
    Ok(())
}

/// The marks of every chunk of a download.
pub(crate) async fn chunk_marks(
    readers: &SqlitePool,
    download_id: DownloadId,
) -> Result<Vec<ChunkMark>> {
    let rows = sqlx::query(
        "SELECT id, source_position, verified FROM chunks WHERE download_id = ? \
         ORDER BY start_offset",
    )
    .bind(download_id.to_string())
    .fetch_all(readers)
    .await?;
    rows.iter()
        .map(|row| {
            Ok(ChunkMark {
                chunk_id: row.get::<String, _>("id").parse()?,
                source_position: row
                    .get::<Option<i64>, _>("source_position")
                    .and_then(|value| u32::try_from(value).ok()),
                verified: row.get::<i64, _>("verified") != 0,
            })
        })
        .collect()
}

/// Notes which source a chunk's bytes come from and whether its pieces were checked.
pub(crate) async fn mark_chunk(
    connection: &mut SqliteConnection,
    chunk_id: ChunkId,
    source_position: Option<u32>,
    verified: bool,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE chunks SET source_position = COALESCE(?, source_position), verified = ?, \
         updated_at = ? WHERE id = ?",
    )
    .bind(source_position.map(i64::from))
    .bind(i64::from(verified))
    .bind(Utc::now())
    .bind(chunk_id.to_string())
    .execute(&mut *connection)
    .await?;
    if result.rows_affected() == 0 {
        bail!(crate::StoreError::not_found("chunk not found"));
    }
    Ok(())
}

/// Moves a chunk's confirmed offset *back* to `committed` and clears its verification.
///
/// The one way an offset goes backwards, and only for bytes a hash proved wrong: an ordinary
/// checkpoint refuses it. `committed` must lie between the chunk's start and its current
/// offset. Answers the download id, so the caller can recompute the download's total.
pub(crate) async fn rewind_chunk(
    connection: &mut SqliteConnection,
    chunk_id: ChunkId,
    committed: u64,
) -> Result<String> {
    let value = i64::try_from(committed).context("chunk offset exceeds SQLite range")?;
    let row =
        sqlx::query("SELECT download_id, start_offset, committed_offset FROM chunks WHERE id = ?")
            .bind(chunk_id.to_string())
            .fetch_optional(&mut *connection)
            .await?
            .context(crate::StoreError::not_found("chunk not found"))?;
    let start: i64 = row.get("start_offset");
    let current: i64 = row.get("committed_offset");
    if value < start || value > current {
        bail!("invalid chunk rewind");
    }
    let download_id: String = row.get("download_id");
    sqlx::query(
        "UPDATE chunks SET committed_offset = ?, verified = 0, updated_at = ? WHERE id = ?",
    )
    .bind(value)
    .bind(Utc::now())
    .bind(chunk_id.to_string())
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "UPDATE downloads SET committed_bytes = (SELECT COALESCE(SUM(committed_offset - \
         start_offset), 0) FROM chunks WHERE download_id = ?), updated_at = ? WHERE id = ?",
    )
    .bind(&download_id)
    .bind(Utc::now())
    .bind(&download_id)
    .execute(&mut *connection)
    .await?;
    Ok(download_id)
}

/// Keeps a checked source set on a LinkGrabber candidate until it is queued.
pub(crate) async fn set_candidate_source_set(
    connection: &mut SqliteConnection,
    candidate_id: CandidateId,
    set: &SourceSet,
) -> Result<()> {
    let result = sqlx::query("UPDATE link_candidates SET source_set_json = ? WHERE id = ?")
        .bind(serde_json::to_string(set)?)
        .bind(candidate_id.to_string())
        .execute(&mut *connection)
        .await?;
    if result.rows_affected() == 0 {
        bail!(crate::StoreError::not_found("candidate not found"));
    }
    Ok(())
}

/// The source set a candidate carries, when it carries one.
///
/// A value that no longer parses reads as none: the candidate is then queued as the single
/// link it also is, which is what it would have been without the set.
pub(crate) async fn candidate_source_set(
    readers: &SqlitePool,
    candidate_id: CandidateId,
) -> Result<Option<SourceSet>> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT source_set_json FROM link_candidates WHERE id = ?")
            .bind(candidate_id.to_string())
            .fetch_optional(readers)
            .await?
            .flatten();
    Ok(raw.and_then(|value| serde_json::from_str(&value).ok()))
}

/// Holds candidates to an address reach: `local_network` when the document came from the
/// person's own hand, the public internet otherwise. A candidate that is gone is skipped.
pub(crate) async fn set_candidates_remote_reach(
    connection: &mut SqliteConnection,
    candidate_ids: &[CandidateId],
    local_network: bool,
) -> Result<()> {
    let reach = if local_network {
        "local_network"
    } else {
        "internet"
    };
    for candidate_id in candidate_ids {
        sqlx::query("UPDATE link_candidates SET remote_reach = ? WHERE id = ?")
            .bind(reach)
            .bind(candidate_id.to_string())
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

/// Every candidate held to an address reach, with whether it may reach the person's own
/// network. A candidate the person added themselves is not in it.
pub(crate) async fn candidates_remote_reach(
    readers: &SqlitePool,
) -> Result<std::collections::HashMap<CandidateId, bool>> {
    let rows =
        sqlx::query("SELECT id, remote_reach FROM link_candidates WHERE remote_reach IS NOT NULL")
            .fetch_all(readers)
            .await?;
    let mut reaches = std::collections::HashMap::with_capacity(rows.len());
    for row in rows {
        let id: String = row.get("id");
        let reach: String = row.get("remote_reach");
        reaches.insert(id.parse()?, reach == "local_network");
    }
    Ok(reaches)
}

/// The address reach one candidate is held to: `Some(true)` for the person's own network,
/// `Some(false)` for the public internet, `None` for a link the person added themselves.
pub(crate) async fn candidate_remote_reach(
    readers: &SqlitePool,
    candidate_id: CandidateId,
) -> Result<Option<bool>> {
    let reach: Option<String> =
        sqlx::query_scalar("SELECT remote_reach FROM link_candidates WHERE id = ?")
            .bind(candidate_id.to_string())
            .fetch_optional(readers)
            .await?
            .flatten();
    Ok(reach.map(|reach| reach == "local_network"))
}
