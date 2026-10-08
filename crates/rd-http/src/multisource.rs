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
//! a time and the others are only failover). Without one, an FTP or SFTP mirror also serves
//! only from the file's first byte: nothing it sends says where its bytes begin (TR-02). With piece hashes, a chunk is checked the moment
//! it is complete: a piece that does not match isolates the source that delivered it, the
//! chunk goes back to the start of that piece, and the file cannot complete until another
//! source delivered the piece correctly. The whole-file hash is checked by the caller before
//! promotion, as for every download.
//!
//! What the run learns about sources and chunks goes to a [`SourceLedger`] as it happens, so a
//! restart starts from the same order, the same backoffs and the same isolated mirrors.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use rd_core::{ChunkId, PieceHashes};
use rd_files::PartFile;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, HttpDownloadError,
    wind_down::wind_down,
};

#[path = "multisource_state.rs"]
mod state;

use state::{
    Fetched, Pool, Tracked, failure_code, first_bad_piece, mirror_offset_unverified,
    no_usable_source, piece_mismatch,
};

/// One address of the file, ready to be fetched from.
#[derive(Clone, Debug)]
pub struct SourceEndpoint {
    /// The source's fixed place in the set, which is how the ledger names it.
    pub position: u32,
    pub url: Url,
    /// Headers for this address only. The caller decides them per address, because a
    /// profile's or an account's credential may belong to one mirror and not to the next.
    pub headers: Vec<(String, String)>,
    /// How an FTP or SFTP mirror is reached; `None` for HTTP, which the engine fetches itself.
    pub via: Option<crate::RangeTransport>,
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
    /// Whether the caller checks a whole-file hash before promotion. With it or with
    /// [`Self::pieces`], an FTP or SFTP mirror may serve a chunk from inside the file.
    pub whole_file_hash: bool,
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
        let hash_basis = request.pieces.is_some() || request.whole_file_hash;
        let part = PartFile::open(request.part_path.clone(), Some(total))
            .await
            .map_err(HttpDownloadError::Local)?;
        let mut run = SourceRun {
            tracked: Arc::new(Tracked {
                ledger: Arc::clone(&ledger),
                confirmed: Mutex::new(HashMap::new()),
            }),
            ledger,
            part,
            part_path: request.part_path,
            total,
            single_chunk: request.chunks.len() == 1,
            pieces: request.pieces,
            pool: Pool {
                sources: request.sources,
                parallel: request.parallel_sources.max(1),
                busy: HashMap::new(),
                out: HashSet::new(),
                hash_basis,
            },
            last_error: None,
            pending: VecDeque::new(),
            tasks: JoinSet::new(),
            // The fetches' own token, so ending them leaves the caller's alone (TR-09).
            workers: cancellation.child_token(),
        };
        run.queue_chunks(request.chunks, &request.unverified)
            .await?;
        loop {
            run.dispatch(self).await?;
            let Some(joined) = run.tasks.join_next().await else {
                if run.pending.is_empty() {
                    break;
                }
                // Chunks are left and no source is: every one of them failed or was
                // isolated during this run, or only mirrors that may not serve them are.
                if run.pool.held_back() {
                    return Err(mirror_offset_unverified());
                }
                return Err(run.last_error.unwrap_or_else(no_usable_source));
            };
            let fetched = match joined {
                Ok(fetched) => fetched,
                Err(error) => {
                    run.wind_down().await;
                    return Err(anyhow::Error::new(error).into());
                }
            };
            run.pool.release(fetched.position);
            if let Some(outcome) = run.settle(fetched).await? {
                return Ok(outcome);
            }
        }
        run.part
            .sync_data()
            .await
            .map_err(HttpDownloadError::Local)?;
        Ok(DownloadOutcome::Complete)
    }
}

/// One multi-source download in progress: its sources, its open chunks and its fetches.
struct SourceRun {
    ledger: Arc<dyn SourceLedger>,
    tracked: Arc<Tracked>,
    part: PartFile,
    part_path: PathBuf,
    total: u64,
    single_chunk: bool,
    pieces: Option<Arc<PieceHashes>>,
    pool: Pool,
    last_error: Option<HttpDownloadError>,
    pending: VecDeque<ChunkSpec>,
    tasks: JoinSet<Fetched>,
    workers: CancellationToken,
}

impl SourceRun {
    /// Queues every incomplete chunk, and checks the complete ones an earlier run never
    /// checked before anything counts them.
    async fn queue_chunks(
        &mut self,
        chunks: Vec<ChunkSpec>,
        unverified: &HashMap<ChunkId, Option<u32>>,
    ) -> Result<(), HttpDownloadError> {
        for mut chunk in chunks {
            if !chunk.is_complete() {
                self.pending.push_back(chunk);
                continue;
            }
            // A chunk finished by an earlier run and never checked. It is checked before
            // anything counts it; the source named for it answers for what it wrote.
            let (Some(pieces), Some(delivered_by)) =
                (self.pieces.as_deref(), unverified.get(&chunk.id))
            else {
                continue;
            };
            match first_bad_piece(&self.part_path, pieces, &chunk, self.total).await? {
                None => self
                    .ledger
                    .chunk_marked(chunk.id, *delivered_by, true)
                    .await
                    .map_err(HttpDownloadError::Internal)?,
                Some(bad) => {
                    // Isolated once, however many of its chunks fail the check.
                    if let Some(position) = delivered_by
                        && self.pool.exclude(*position)
                    {
                        self.ledger
                            .source_isolated(*position, rd_core::CODE_PIECE_MISMATCH)
                            .await
                            .map_err(HttpDownloadError::Internal)?;
                    }
                    self.ledger
                        .chunk_rewound(chunk.id, bad)
                        .await
                        .map_err(HttpDownloadError::Internal)?;
                    chunk.committed = bad;
                    self.pending.push_back(chunk);
                }
            }
        }
        Ok(())
    }

    /// Hands every pending chunk a source while one is free. A chunk no free source may
    /// serve waits, in its place, without holding up the ones behind it.
    async fn dispatch(&mut self, engine: &DownloadEngine) -> Result<(), HttpDownloadError> {
        let mut waiting = VecDeque::new();
        while let Some(chunk) = self.pending.pop_front() {
            let Some(source) = self.pool.pick(chunk.committed) else {
                waiting.push_back(chunk);
                continue;
            };
            // Named before a byte arrives, so a restart that finds the chunk complete and
            // unchecked knows whom to hold to account for it.
            if let Err(error) = self
                .ledger
                .chunk_marked(chunk.id, Some(source.position), false)
                .await
            {
                self.wind_down().await;
                return Err(HttpDownloadError::Internal(error));
            }
            self.spawn_fetch(engine, chunk, source);
        }
        self.pending = waiting;
        Ok(())
    }

    fn spawn_fetch(&mut self, engine: &DownloadEngine, chunk: ChunkSpec, source: SourceEndpoint) {
        let covers_whole_file =
            self.single_chunk && chunk.start == 0 && chunk.end == Some(self.total);
        let engine = engine.clone();
        let part = self.part.clone();
        let checkpoints: Arc<dyn CheckpointSink> = self.tracked.clone();
        let cancellation = self.workers.clone();
        let total = self.total;
        self.tasks.spawn(async move {
            let started_at = chunk.committed;
            let result = match &source.via {
                Some(via) => {
                    engine
                        .fetch_via(via, part, checkpoints, cancellation, chunk.clone())
                        .await
                }
                None => {
                    engine
                        .fetch_chunk(
                            source.url.clone(),
                            Arc::new(source.headers.clone()),
                            part,
                            checkpoints,
                            cancellation,
                            chunk.clone(),
                            covers_whole_file,
                            total,
                        )
                        .await
                }
            };
            Fetched {
                chunk,
                position: source.position,
                started_at,
                result,
            }
        });
    }

    /// Books one finished fetch. `Some` ends the run with that outcome, its fetches already
    /// wound down.
    async fn settle(
        &mut self,
        fetched: Fetched,
    ) -> Result<Option<DownloadOutcome>, HttpDownloadError> {
        let Fetched {
            chunk,
            position,
            started_at,
            result,
        } = fetched;
        match result {
            Ok(DownloadOutcome::Complete) => {
                self.settle_complete(chunk, position, started_at).await?;
            }
            Ok(DownloadOutcome::Paused) => {
                self.wind_down().await;
                return Ok(Some(DownloadOutcome::Paused));
            }
            // This machine's disk, not the mirror: another source writes to the same one.
            Err(HttpDownloadError::Local(error)) => {
                self.wind_down().await;
                return Err(HttpDownloadError::Local(error));
            }
            Err(HttpDownloadError::Internal(error)) => {
                self.wind_down().await;
                return Err(HttpDownloadError::Internal(error));
            }
            Err(error) => {
                self.settle_failure(chunk, position, started_at, error)
                    .await?;
            }
        }
        Ok(None)
    }

    /// A chunk every byte of which arrived: checked against its pieces, then booked to its
    /// source, or rewound to the refused piece with the source isolated.
    async fn settle_complete(
        &mut self,
        mut chunk: ChunkSpec,
        position: u32,
        started_at: u64,
    ) -> Result<(), HttpDownloadError> {
        let end = chunk.end.unwrap_or(self.total);
        chunk.committed = end;
        // Every byte of the chunk is confirmed and none of it is checked yet: the
        // window a restart has to close by checking before it builds on the chunk.
        rd_core::failpoint!("http.before_piece_check", || {
            HttpDownloadError::Failure(
                rd_core::Failure::coded(
                    rd_core::FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    "download.crash_point",
                    "crash point: http.before_piece_check",
                )
                .with_param("point", "http.before_piece_check"),
            )
        });
        let verdict = match self.pieces.as_deref() {
            Some(pieces) => first_bad_piece(&self.part_path, pieces, &chunk, self.total).await?,
            None => None,
        };
        let outcome = match verdict {
            None => {
                self.ledger
                    .chunk_marked(chunk.id, Some(position), self.pieces.is_some())
                    .await?;
                self.ledger
                    .source_delivered(position, end.saturating_sub(started_at))
                    .await
            }
            Some(bad) => {
                tracing::warn!(
                    position,
                    offset = bad,
                    "a mirror delivered a piece that does not match its hash"
                );
                if self.pool.exclude(position) {
                    self.ledger
                        .source_isolated(position, rd_core::CODE_PIECE_MISMATCH)
                        .await?;
                }
                self.ledger.chunk_rewound(chunk.id, bad).await?;
                self.tracked.record(chunk.id, bad);
                chunk.committed = bad;
                self.pending.push_back(chunk);
                self.last_error = Some(piece_mismatch(bad));
                Ok(())
            }
        };
        if let Err(error) = outcome {
            self.wind_down().await;
            return Err(HttpDownloadError::Internal(error));
        }
        Ok(())
    }

    /// A source that failed: what it confirmed stays confirmed, and the chunk goes on.
    async fn settle_failure(
        &mut self,
        mut chunk: ChunkSpec,
        position: u32,
        started_at: u64,
        error: HttpDownloadError,
    ) -> Result<(), HttpDownloadError> {
        // The source failed. What it confirmed stays confirmed; the chunk goes to
        // the next source from there.
        let confirmed = self
            .tracked
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
        self.pool.exclude(position);
        let ledger = &self.ledger;
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
            self.wind_down().await;
            return Err(HttpDownloadError::Internal(error));
        }
        chunk.committed = confirmed;
        self.pending.push_back(chunk);
        self.last_error = Some(error);
        Ok(())
    }

    /// Stops every fetch still running, without touching the caller's token.
    async fn wind_down(&mut self) {
        wind_down(&mut self.tasks, &self.workers).await;
    }
}

#[cfg(test)]
#[path = "multisource_tests.rs"]
mod tests;
