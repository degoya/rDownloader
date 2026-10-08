//! Concurrent range download executor: the request types and the run over one file's chunks.
//!
//! One worker per chunk; its phases live in submodules — the request and the checks its
//! response passes (`request`), where the body belongs (`resume`), writing and
//! checkpointing it (`write`), and how a failure is classified (`failure`).

mod failure;
mod request;
mod resume;
#[cfg(test)]
mod tests;
mod worker;
mod write;

use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use rd_core::{ChunkId, Failure};
use rd_files::PartFile;
use reqwest::Client;
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use rd_limits::ScopedLimiter;

use crate::{
    ChunkSpec,
    hostlimit::HostLimits,
    transform::{StreamTransform, TransformCheckpoint, plan_resume},
    wind_down::wind_down,
};

pub(crate) use failure::{network_failure, status_failure};
use resume::layout_covers_file;
use worker::Worker;

pub(crate) const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);
pub(crate) const CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;

/// Durable checkpoint target implemented by the scheduler/database adapter.
#[async_trait]
pub trait CheckpointSink: Send + Sync {
    async fn commit(&self, chunk_id: ChunkId, committed_offset: u64) -> anyhow::Result<()>;

    /// Records one finished provider-chunk MAC of a transformed stream (RD-110-33).
    ///
    /// Defaulted to nothing because an ordinary download has no transform and therefore no
    /// chunk MACs; only a caller that can hand a [`DownloadRequest::transform`] back on the
    /// next attempt needs to implement it.
    async fn commit_chunk_mac(&self, _index: u64, _mac: [u8; 16]) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Fully resolved HTTP transfer request.
pub struct DownloadRequest {
    pub url: Url,
    pub part_path: PathBuf,
    pub total_bytes: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub use_ranges: bool,
    pub chunks: Vec<ChunkSpec>,
    /// Headers a resolver bound to the transfer (Referer, …); replayed on every chunk.
    pub headers: Vec<(String, String)>,
    /// `GET` unless a consented replay template says otherwise.
    pub method: rd_core::ReplayMethod,
    /// Body of a replayed POST, re-sent on every attempt.
    pub body: Option<ReplayPayload>,
    /// Origins this transfer may talk to. Empty for an ordinary download, which is not
    /// origin-restricted.
    pub approved_origins: Arc<Vec<String>>,
    /// User agent the browser used, applied per request so it cannot leak into unrelated
    /// transfers sharing the same pooled client.
    pub captured_user_agent: Option<String>,
    /// How this provider's bytes become the file, and what a previous attempt already
    /// accounted for (RD-110-33). `None` -- every ordinary download -- runs exactly the code
    /// it ran before this existed.
    pub transform: Option<TransformPlan>,
}

/// A validated transform and the state a continuation may build on.
pub struct TransformPlan {
    pub transform: Arc<StreamTransform>,
    pub checkpoint: TransformCheckpoint,
}

impl DownloadRequest {
    /// An ordinary unrestricted `GET`, the shape every non-replay caller wants.
    #[must_use]
    pub fn get(
        url: Url,
        part_path: PathBuf,
        total_bytes: Option<u64>,
        chunks: Vec<ChunkSpec>,
    ) -> Self {
        Self {
            url,
            part_path,
            total_bytes,
            etag: None,
            last_modified: None,
            use_ranges: false,
            chunks,
            headers: Vec::new(),
            method: rd_core::ReplayMethod::Get,
            body: None,
            approved_origins: Arc::new(Vec::new()),
            captured_user_agent: None,
            transform: None,
        }
    }
}

/// Body of a replayed request.
///
/// `Bytes` is reference counted, which matters because the body has to be re-sent on every
/// retry and on the resume request.
#[derive(Clone, Debug)]
pub struct ReplayPayload {
    pub content_type: String,
    pub bytes: bytes::Bytes,
}

/// Terminal result of one engine invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadOutcome {
    Complete,
    Paused,
}

/// Failures requiring scheduler policy rather than blind IO retries.
#[derive(Debug, Error)]
pub enum HttpDownloadError {
    #[error("server ignored a required range request")]
    RangeIgnored,
    #[error("remote object changed while resuming")]
    RemoteChanged,
    #[error(transparent)]
    Failure(#[from] Failure),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
    /// The transfer failed on this machine, not at the hoster: the staging file could not be
    /// opened, written or synced — no space, no permission, a detached share.
    ///
    /// Told apart from [`Self::Internal`] on purpose. Both used to be reported as an
    /// ordinary transient failure, which made a full disk indistinguishable from a flaky
    /// server — and a download with mirrors then gave up on the hoster and tried the next
    /// one, which writes to the very same disk (RD-110-20).
    #[error("the file could not be written on this machine: {0}")]
    Local(#[source] anyhow::Error),
}

/// Stable code of [`HttpDownloadError::Local`], read by the queue to keep a local cause from
/// moving a download to another mirror.
pub const LOCAL_IO_CODE: &str = "download.local_io";

/// Concurrent range download executor.
#[derive(Clone)]
pub struct DownloadEngine {
    client: Client,
    limiter: ScopedLimiter,
    hosts: HostLimits,
}

impl DownloadEngine {
    /// Creates an engine around an isolated client and shared limiter.
    #[must_use]
    pub fn new(client: Client, limiter: ScopedLimiter) -> Self {
        Self {
            client,
            limiter,
            hosts: HostLimits::default(),
        }
    }

    /// The pacing every byte this engine fetches goes through, whoever fetches it.
    pub(crate) const fn limiter(&self) -> &ScopedLimiter {
        &self.limiter
    }

    /// Shares one connection policy with every other transfer.
    ///
    /// Without this an engine limits only its own chunks, which is half the problem: the
    /// connections a host sees come from every running file at once.
    #[must_use]
    pub fn with_host_limits(mut self, hosts: HostLimits) -> Self {
        self.hosts = hosts;
        self
    }

    /// Downloads every incomplete chunk and persists only post-sync offsets.
    pub async fn download(
        &self,
        mut request: DownloadRequest,
        checkpoints: Arc<dyn CheckpointSink>,
        cancellation: CancellationToken,
    ) -> Result<DownloadOutcome, HttpDownloadError> {
        let part = PartFile::open(request.part_path, request.total_bytes)
            .await
            .map_err(HttpDownloadError::Local)?;
        // The transform has the first word on the chunk layout: a MAC chain is sequential
        // inside a provider chunk, so a connection that starts in the middle of one cannot
        // compute it. Where the layout does not line up, the run gives up its parallelism
        // rather than condensing something wrong.
        let (transform, adopted) = match request.transform {
            None => (None, std::collections::BTreeMap::new()),
            Some(plan) => {
                let resume = plan_resume(
                    &plan.transform,
                    request.chunks,
                    &plan.checkpoint,
                    request.total_bytes,
                );
                if resume.collapsed {
                    tracing::info!(
                        "this stream's chunk boundaries do not line up with the provider's; \
                         falling back to a single connection"
                    );
                }
                if resume.restarted {
                    tracing::info!(
                        "the recorded chunk MACs were written by a different transform \
                         description; starting this file over"
                    );
                }
                request.chunks = resume.chunks;
                (Some(plan.transform), resume.macs)
            }
        };
        let macs = Arc::new(std::sync::Mutex::new(adopted));
        // The integrity value covers the whole file, so it can only be checked by a run that
        // is responsible for the whole file. A caller that hands over part of the layout --
        // and the scheduler never does -- gets its bytes transformed and no verdict, rather
        // than a refusal for chunks nobody asked it to fetch.
        let verify_at_end = transform.as_ref().is_some_and(|transform| {
            layout_covers_file(
                &request.chunks,
                request.total_bytes.or_else(|| transform.expected_size()),
            )
        });
        let ranged = request.use_ranges;
        if request.chunks.len() > 1 && !ranged {
            return Err(HttpDownloadError::RangeIgnored);
        }
        if !ranged && request.chunks.iter().any(|chunk| chunk.committed > 0) {
            return Err(HttpDownloadError::RangeIgnored);
        }
        // A POST is not a safe idempotent GET: issuing it N times in parallel could trigger
        // the server-side side effect N times, and no specification defines range semantics
        // for it. One chunk, always.
        if request.method == rd_core::ReplayMethod::Post && request.chunks.len() > 1 {
            return Err(HttpDownloadError::RangeIgnored);
        }

        let mut tasks = JoinSet::new();
        // The workers' own token: one worker's pause or failure stops its siblings without
        // cancelling the caller's token, which is the queue's and not this transfer's to end.
        let workers = cancellation.child_token();
        let headers = Arc::new(request.headers);
        // Whether a single chunk stands for the entire file. Only then is a server that
        // answers a range request with the whole body still usable: the bytes it sends are
        // exactly the bytes this worker is responsible for.
        let single_chunk = request.chunks.len() == 1;
        let total_bytes = request.total_bytes;
        for chunk in request
            .chunks
            .into_iter()
            .filter(|chunk| !chunk.is_complete())
        {
            let covers_whole_file = single_chunk
                && chunk.start == 0
                && (chunk.end.is_none() || chunk.end == total_bytes);
            let worker = Worker {
                client: self.client.clone(),
                limiter: self.limiter.clone(),
                hosts: self.hosts.clone(),
                covers_whole_file,
                part: part.clone(),
                checkpoints: Arc::clone(&checkpoints),
                cancellation: workers.clone(),
                url: request.url.clone(),
                validator: request
                    .etag
                    .clone()
                    .or_else(|| request.last_modified.clone()),
                total_bytes,
                require_range: ranged,
                headers: Arc::clone(&headers),
                method: request.method,
                body: request.body.clone(),
                approved_origins: Arc::clone(&request.approved_origins),
                captured_user_agent: request.captured_user_agent.clone(),
                transform: transform.clone(),
                macs: Arc::clone(&macs),
            };
            tasks.spawn(async move { worker.run(chunk).await });
        }

        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(DownloadOutcome::Complete)) => {}
                Ok(Ok(DownloadOutcome::Paused)) => {
                    wind_down(&mut tasks, &workers).await;
                    return Ok(DownloadOutcome::Paused);
                }
                Ok(Err(error)) => {
                    wind_down(&mut tasks, &workers).await;
                    return Err(error);
                }
                Err(error) => {
                    wind_down(&mut tasks, &workers).await;
                    return Err(anyhow::Error::new(error).into());
                }
            }
        }
        part.sync_data().await.map_err(HttpDownloadError::Local)?;
        // Before the caller promotes the part file, and only here: a wrong key produces a
        // file of exactly the right length under exactly the right name, and this is the one
        // thing that tells it apart from a correct download. A mismatch is a failed attempt,
        // so the partial file stays where it is and nothing is presented as complete.
        if let Some(transform) = &transform
            && verify_at_end
        {
            let finished = macs
                .lock()
                .map_err(|_| anyhow::anyhow!("mac state"))?
                .clone();
            transform.verify(&finished)?;
        }
        Ok(DownloadOutcome::Complete)
    }
}

impl DownloadEngine {
    /// Fetches one chunk from one address: a plain `GET` with a range, no transform, no
    /// replay and no validator. The shape every source of a mirror set is fetched in
    /// (RD-150-03), where each source has validators of its own and the piece and whole-file
    /// hashes stand in for them. The set's length still holds every answer to it (TR-01).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fetch_chunk(
        &self,
        url: Url,
        headers: Arc<Vec<(String, String)>>,
        part: PartFile,
        checkpoints: Arc<dyn CheckpointSink>,
        cancellation: CancellationToken,
        chunk: ChunkSpec,
        covers_whole_file: bool,
        total_bytes: u64,
    ) -> Result<DownloadOutcome, HttpDownloadError> {
        Worker {
            client: self.client.clone(),
            limiter: self.limiter.clone(),
            hosts: self.hosts.clone(),
            covers_whole_file,
            part,
            checkpoints,
            cancellation,
            url,
            validator: None,
            total_bytes: Some(total_bytes),
            require_range: true,
            headers,
            method: rd_core::ReplayMethod::Get,
            body: None,
            approved_origins: Arc::new(Vec::new()),
            captured_user_agent: None,
            transform: None,
            macs: Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new())),
        }
        .run(chunk)
        .await
    }
}
