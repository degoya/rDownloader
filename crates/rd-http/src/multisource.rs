//! One file from several sources at once (RD-150-03).
//!
//! A Metalink document names the same bytes at several addresses. This runs the chunks of one
//! part file against those addresses: each chunk is fetched from one source, chunks are spread
//! over the sources in their order, and a source that fails hands its chunk to the next one
//! **from the last confirmed offset** — the bytes a checkpoint confirmed are never fetched
//! again, whoever delivered them.
//!
//! Mixing sources is only safe when something proves the bytes. The caller decides that
//! ([`MultiSourceRequest::parallel_sources`] is 1 without a hash basis, so one source works at
//! a time and the others are only failover). With piece hashes, a chunk is checked the moment
//! it is complete: a piece that does not match isolates the source that delivered it, the
//! chunk goes back to the start of that piece, and the file cannot complete until another
//! source delivered the piece correctly. The whole-file hash is checked by the caller before
//! promotion, as for every download.
//!
//! What the run learns about sources and chunks goes to a [`SourceLedger`] as it happens, so a
//! restart starts from the same order, the same backoffs and the same isolated mirrors.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{ChunkId, Failure, FailureKind, PieceHashes};
use rd_files::PartFile;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, HttpDownloadError};

/// One address of the file, ready to be fetched from.
#[derive(Clone, Debug)]
pub struct SourceEndpoint {
    /// The source's fixed place in the set, which is how the ledger names it.
    pub position: u32,
    pub url: Url,
    /// Headers for this address only. The caller decides them per address, because a
    /// profile's or an account's credential may belong to one mirror and not to the next.
    pub headers: Vec<(String, String)>,
}

/// A file, its chunks and the sources to fetch them from.
pub struct MultiSourceRequest {
    pub part_path: PathBuf,
    pub total_bytes: u64,
    pub chunks: Vec<ChunkSpec>,
    /// The usable sources, in the order they are tried.
    pub sources: Vec<SourceEndpoint>,
    /// How many of the sources fetch at the same time; 1 means one at a time, the rest only
    /// standing by. Clamped to at least 1.
    pub parallel_sources: usize,
    /// Piece hashes to check every completed chunk against. `None` when the set stated none
    /// or the chunk layout does not fall on piece boundaries.
    pub pieces: Option<Arc<PieceHashes>>,
    /// Chunks that are complete but were never checked — the window between the last byte
    /// and the check, crossed by a restart — with the source that delivered them.
    pub unverified: HashMap<ChunkId, Option<u32>>,
}

/// Where a multi-source run writes down what it learns.
#[async_trait]
pub trait SourceLedger: CheckpointSink {
    /// Confirmed bytes arrived from this source.
    async fn source_delivered(&self, position: u32, bytes: u64) -> anyhow::Result<()>;
    /// The source failed with this stable code and waits before it is tried again.
    async fn source_failed(
        &self,
        position: u32,
        code: &str,
        retry_after_seconds: Option<u64>,
    ) -> anyhow::Result<()>;
    /// The source delivered bytes a hash refused; it is not tried again.
    async fn source_isolated(&self, position: u32, code: &str) -> anyhow::Result<()>;
    /// Which source a chunk's bytes came from, and whether its pieces were checked.
    async fn chunk_marked(
        &self,
        chunk_id: ChunkId,
        position: Option<u32>,
        verified: bool,
    ) -> anyhow::Result<()>;
    /// Moves a chunk's confirmed offset back to the start of a refused piece.
    async fn chunk_rewound(&self, chunk_id: ChunkId, committed: u64) -> anyhow::Result<()>;
}

/// Passes checkpoints on and remembers the last confirmed offset of every chunk, so a chunk
/// whose source failed is handed on from exactly there.
struct Tracked {
    ledger: Arc<dyn SourceLedger>,
    confirmed: Mutex<HashMap<ChunkId, u64>>,
}

impl Tracked {
    fn confirmed(&self, chunk_id: ChunkId) -> Option<u64> {
        self.confirmed
            .lock()
            .ok()
            .and_then(|confirmed| confirmed.get(&chunk_id).copied())
    }

    fn record(&self, chunk_id: ChunkId, committed: u64) {
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
struct Pool {
    sources: Vec<SourceEndpoint>,
    parallel: usize,
    busy: HashMap<u32, usize>,
    out: HashSet<u32>,
}

impl Pool {
    /// The source for the next chunk: among the first `parallel` sources still in, the one
    /// with the fewest chunks in flight, earlier ones first on a tie. That is what spreads
    /// consecutive chunks over two mirrors instead of stacking them on the first.
    fn pick(&mut self) -> Option<SourceEndpoint> {
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

    fn release(&mut self, position: u32) {
        if let Some(count) = self.busy.get_mut(&position) {
            *count = count.saturating_sub(1);
        }
    }

    /// Takes a source out of this run; `false` when it already was.
    fn exclude(&mut self, position: u32) -> bool {
        self.out.insert(position)
    }
}

/// What one fetch task hands back.
struct Fetched {
    chunk: ChunkSpec,
    position: u32,
    /// Confirmed offset when the fetch started, to count what this source delivered.
    started_at: u64,
    result: Result<DownloadOutcome, HttpDownloadError>,
}

impl DownloadEngine {
    /// Downloads every incomplete chunk of a file from a set of sources (RD-150-03).
    ///
    /// Ends `Complete` only when every chunk is complete and, with piece hashes, checked;
    /// `Paused` on cancellation; an error when a local write failed or no source is left
    /// for a chunk that still needs one — the last source's failure, so the queue's retry
    /// policy sees the real cause.
    pub async fn download_from_sources(
        &self,
        request: MultiSourceRequest,
        ledger: Arc<dyn SourceLedger>,
        cancellation: CancellationToken,
    ) -> Result<DownloadOutcome, HttpDownloadError> {
        let total = request.total_bytes;
        let part = PartFile::open(request.part_path.clone(), Some(total))
            .await
            .map_err(HttpDownloadError::Local)?;
        let tracked = Arc::new(Tracked {
            ledger: Arc::clone(&ledger),
            confirmed: Mutex::new(HashMap::new()),
        });
        let single_chunk = request.chunks.len() == 1;
        let pieces = request.pieces.clone();
        let mut pool = Pool {
            sources: request.sources.clone(),
            parallel: request.parallel_sources.max(1),
            busy: HashMap::new(),
            out: HashSet::new(),
        };
        let mut last_error: Option<HttpDownloadError> = None;
        let mut pending: VecDeque<ChunkSpec> = VecDeque::new();
        for mut chunk in request.chunks {
            if !chunk.is_complete() {
                pending.push_back(chunk);
                continue;
            }
            // A chunk finished by an earlier run and never checked. It is checked before
            // anything counts it; the source named for it answers for what it wrote.
            let (Some(pieces), Some(delivered_by)) =
                (pieces.as_deref(), request.unverified.get(&chunk.id))
            else {
                continue;
            };
            match first_bad_piece(&request.part_path, pieces, &chunk, total).await? {
                None => ledger
                    .chunk_marked(chunk.id, *delivered_by, true)
                    .await
                    .map_err(HttpDownloadError::Internal)?,
                Some(bad) => {
                    // Isolated once, however many of its chunks fail the check.
                    if let Some(position) = delivered_by
                        && pool.exclude(*position)
                    {
                        ledger
                            .source_isolated(*position, rd_core::CODE_PIECE_MISMATCH)
                            .await
                            .map_err(HttpDownloadError::Internal)?;
                    }
                    ledger
                        .chunk_rewound(chunk.id, bad)
                        .await
                        .map_err(HttpDownloadError::Internal)?;
                    chunk.committed = bad;
                    pending.push_back(chunk);
                }
            }
        }

        let mut tasks: JoinSet<Fetched> = JoinSet::new();
        loop {
            while let Some(chunk) = pending.pop_front() {
                let Some(source) = pool.pick() else {
                    pending.push_front(chunk);
                    break;
                };
                // Named before a byte arrives, so a restart that finds the chunk complete and
                // unchecked knows whom to hold to account for it.
                if let Err(error) = ledger
                    .chunk_marked(chunk.id, Some(source.position), false)
                    .await
                {
                    cancellation.cancel();
                    tasks.abort_all();
                    return Err(HttpDownloadError::Internal(error));
                }
                let covers_whole_file =
                    single_chunk && chunk.start == 0 && chunk.end == Some(total);
                let engine = self.clone();
                let part = part.clone();
                let checkpoints: Arc<dyn CheckpointSink> = tracked.clone();
                let cancellation = cancellation.clone();
                tasks.spawn(async move {
                    let started_at = chunk.committed;
                    let result = engine
                        .fetch_chunk(
                            source.url.clone(),
                            Arc::new(source.headers.clone()),
                            part,
                            checkpoints,
                            cancellation,
                            chunk.clone(),
                            covers_whole_file,
                        )
                        .await;
                    Fetched {
                        chunk,
                        position: source.position,
                        started_at,
                        result,
                    }
                });
            }
            let Some(joined) = tasks.join_next().await else {
                if pending.is_empty() {
                    break;
                }
                // Chunks are left and no source is: every one of them failed or was
                // isolated during this run.
                return Err(last_error.unwrap_or_else(no_usable_source));
            };
            let fetched = match joined {
                Ok(fetched) => fetched,
                Err(error) => {
                    cancellation.cancel();
                    tasks.abort_all();
                    return Err(anyhow::Error::new(error).into());
                }
            };
            pool.release(fetched.position);
            let Fetched {
                mut chunk,
                position,
                started_at,
                result,
            } = fetched;
            match result {
                Ok(DownloadOutcome::Complete) => {
                    let end = chunk.end.unwrap_or(total);
                    chunk.committed = end;
                    // Every byte of the chunk is confirmed and none of it is checked yet: the
                    // window a restart has to close by checking before it builds on the chunk.
                    rd_core::failpoint!("http.before_piece_check", || {
                        HttpDownloadError::Failure(
                            Failure::coded(
                                FailureKind::Transient {
                                    retry_after_seconds: None,
                                },
                                "download.crash_point",
                                "crash point: http.before_piece_check",
                            )
                            .with_param("point", "http.before_piece_check"),
                        )
                    });
                    let verdict = match pieces.as_deref() {
                        Some(pieces) => {
                            first_bad_piece(&request.part_path, pieces, &chunk, total).await?
                        }
                        None => None,
                    };
                    let outcome = match verdict {
                        None => {
                            ledger
                                .chunk_marked(chunk.id, Some(position), pieces.is_some())
                                .await?;
                            ledger
                                .source_delivered(position, end.saturating_sub(started_at))
                                .await
                        }
                        Some(bad) => {
                            tracing::warn!(
                                position,
                                offset = bad,
                                "a mirror delivered a piece that does not match its hash"
                            );
                            if pool.exclude(position) {
                                ledger
                                    .source_isolated(position, rd_core::CODE_PIECE_MISMATCH)
                                    .await?;
                            }
                            ledger.chunk_rewound(chunk.id, bad).await?;
                            tracked.record(chunk.id, bad);
                            chunk.committed = bad;
                            pending.push_back(chunk);
                            last_error = Some(piece_mismatch(bad));
                            Ok(())
                        }
                    };
                    if let Err(error) = outcome {
                        cancellation.cancel();
                        tasks.abort_all();
                        return Err(HttpDownloadError::Internal(error));
                    }
                }
                Ok(DownloadOutcome::Paused) => {
                    cancellation.cancel();
                    tasks.abort_all();
                    return Ok(DownloadOutcome::Paused);
                }
                // This machine's disk, not the mirror: another source writes to the same one.
                Err(HttpDownloadError::Local(error)) => {
                    cancellation.cancel();
                    tasks.abort_all();
                    return Err(HttpDownloadError::Local(error));
                }
                Err(HttpDownloadError::Internal(error)) => {
                    cancellation.cancel();
                    tasks.abort_all();
                    return Err(HttpDownloadError::Internal(error));
                }
                Err(error) => {
                    // The source failed. What it confirmed stays confirmed; the chunk goes to
                    // the next source from there.
                    let confirmed = tracked
                        .confirmed(chunk.id)
                        .unwrap_or(chunk.committed)
                        .max(chunk.committed);
                    let (code, retry_after) = failure_code(&error);
                    tracing::info!(
                        position,
                        code,
                        confirmed,
                        "a mirror failed; its chunk moves on to the next source"
                    );
                    pool.exclude(position);
                    let recorded = async {
                        if confirmed > started_at {
                            ledger
                                .source_delivered(position, confirmed - started_at)
                                .await?;
                        }
                        // A source that turned out to point inside the network — a name that
                        // answered differently at connect time, a redirect to a literal
                        // address — is not tried again.
                        if code == rd_core::CODE_INTERNAL_ADDRESS {
                            ledger.source_isolated(position, code).await
                        } else {
                            ledger.source_failed(position, code, retry_after).await
                        }
                    }
                    .await;
                    if let Err(error) = recorded {
                        cancellation.cancel();
                        tasks.abort_all();
                        return Err(HttpDownloadError::Internal(error));
                    }
                    chunk.committed = confirmed;
                    pending.push_back(chunk);
                    last_error = Some(error);
                }
            }
        }
        part.sync_data().await.map_err(HttpDownloadError::Local)?;
        Ok(DownloadOutcome::Complete)
    }
}

/// The start of the first piece inside `chunk` whose bytes do not match, or `None`.
///
/// Only pieces wholly inside the chunk are checked; the scheduler plans chunks on piece
/// boundaries whenever pieces exist, so that is every piece.
async fn first_bad_piece(
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
fn failure_code(error: &HttpDownloadError) -> (&str, Option<u64>) {
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

fn piece_mismatch(offset: u64) -> HttpDownloadError {
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

fn no_usable_source() -> HttpDownloadError {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        rd_core::CODE_NO_USABLE_SOURCE,
        "no source of this file can be used right now",
    )
    .into()
}

#[cfg(test)]
#[path = "multisource_tests.rs"]
mod tests;
