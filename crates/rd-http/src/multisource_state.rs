//! What a multi-source run keeps track of: the confirmed offset of every chunk, which sources
//! are free, busy or out, and how a refused piece or a failed source is judged.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{ChunkId, Failure, FailureKind, PieceHashes};

use crate::{CheckpointSink, ChunkSpec, DownloadOutcome, HttpDownloadError};

use super::{SourceEndpoint, SourceLedger};

/// Passes checkpoints on and remembers the last confirmed offset of every chunk, so a chunk
/// whose source failed is handed on from exactly there.
pub(super) struct Tracked {
    pub(super) ledger: Arc<dyn SourceLedger>,
    pub(super) confirmed: Mutex<HashMap<ChunkId, u64>>,
}

impl Tracked {
    pub(super) fn confirmed(&self, chunk_id: ChunkId) -> Option<u64> {
        self.confirmed
            .lock()
            .ok()
            .and_then(|confirmed| confirmed.get(&chunk_id).copied())
    }

    pub(super) fn record(&self, chunk_id: ChunkId, committed: u64) {
        if let Ok(mut confirmed) = self.confirmed.lock() {
            confirmed.insert(chunk_id, committed);
        }
    }
}

#[async_trait]
impl CheckpointSink for Tracked {
    async fn commit(&self, chunk_id: ChunkId, committed_offset: u64) -> anyhow::Result<()> {
        self.ledger.commit(chunk_id, committed_offset).await?;
        self.record(chunk_id, committed_offset);
        Ok(())
    }
}

/// Which sources are free, busy or out for the rest of this run.
pub(super) struct Pool {
    pub(super) sources: Vec<SourceEndpoint>,
    pub(super) parallel: usize,
    pub(super) busy: HashMap<u32, usize>,
    pub(super) out: HashSet<u32>,
}

impl Pool {
    /// The source for the next chunk: among the first `parallel` sources still in, the one
    /// with the fewest chunks in flight, earlier ones first on a tie. That is what spreads
    /// consecutive chunks over two mirrors instead of stacking them on the first.
    pub(super) fn pick(&mut self) -> Option<SourceEndpoint> {
        let chosen = self
            .sources
            .iter()
            .filter(|source| !self.out.contains(&source.position))
            .take(self.parallel)
            .min_by_key(|source| self.busy.get(&source.position).copied().unwrap_or(0))?
            .clone();
        *self.busy.entry(chosen.position).or_default() += 1;
        Some(chosen)
    }

    pub(super) fn release(&mut self, position: u32) {
        if let Some(count) = self.busy.get_mut(&position) {
            *count = count.saturating_sub(1);
        }
    }

    /// Takes a source out of this run; `false` when it already was.
    pub(super) fn exclude(&mut self, position: u32) -> bool {
        self.out.insert(position)
    }
}

/// What one fetch task hands back.
pub(super) struct Fetched {
    pub(super) chunk: ChunkSpec,
    pub(super) position: u32,
    /// Confirmed offset when the fetch started, to count what this source delivered.
    pub(super) started_at: u64,
    pub(super) result: Result<DownloadOutcome, HttpDownloadError>,
}

/// The start of the first piece inside `chunk` whose bytes do not match, or `None`.
///
/// Only pieces wholly inside the chunk are checked; the scheduler plans chunks on piece
/// boundaries whenever pieces exist, so that is every piece.
pub(super) async fn first_bad_piece(
    path: &Path,
    pieces: &PieceHashes,
    chunk: &ChunkSpec,
    total: u64,
) -> Result<Option<u64>, HttpDownloadError> {
    let end = chunk.end.unwrap_or(total);
    let length = pieces.length.max(1);
    let first = usize::try_from(chunk.start.div_ceil(length)).unwrap_or(usize::MAX);
    for index in first..pieces.hashes.len() {
        let (start, stop) = pieces.range(index, total);
        if stop > end || start >= stop {
            break;
        }
        let digest = rd_files::checksum_range(path, pieces.algorithm, start, stop - start)
            .await
            .map_err(HttpDownloadError::Local)?;
        if !digest.eq_ignore_ascii_case(&pieces.hashes[index]) {
            return Ok(Some(start));
        }
    }
    Ok(None)
}

/// The stable code and the server's requested wait behind a source's failure.
pub(super) fn failure_code(error: &HttpDownloadError) -> (&str, Option<u64>) {
    match error {
        HttpDownloadError::RangeIgnored => ("download.range_ignored", None),
        HttpDownloadError::RemoteChanged => ("download.remote_changed", None),
        HttpDownloadError::Failure(failure) => {
            let retry_after = match failure.category {
                FailureKind::RateLimited {
                    retry_after_seconds,
                }
                | FailureKind::Transient {
                    retry_after_seconds,
                } => retry_after_seconds,
                _ => None,
            };
            (
                failure.code.as_deref().unwrap_or("download.failed"),
                retry_after,
            )
        }
        HttpDownloadError::Internal(_) | HttpDownloadError::Local(_) => ("download.failed", None),
    }
}

pub(super) fn piece_mismatch(offset: u64) -> HttpDownloadError {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        rd_core::CODE_PIECE_MISMATCH,
        format!("a mirror delivered a piece at byte {offset} that does not match its hash"),
    )
    .with_param("offset", offset)
    .into()
}

pub(super) fn no_usable_source() -> HttpDownloadError {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        rd_core::CODE_NO_USABLE_SOURCE,
        "no source of this file can be used right now",
    )
    .into()
}
