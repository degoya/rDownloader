use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use rd_core::{ChunkId, Failure, FailureKind};
use rd_files::PartFile;
use reqwest::{Client, StatusCode, header};
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use rd_limits::ScopedLimiter;

use crate::{
    ChunkSpec,
    hostlimit::HostLimits,
    transform::{MacWalker, StreamTransform, TransformCheckpoint, plan_resume},
};

const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);
const CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;

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
                cancellation: cancellation.clone(),
                url: request.url.clone(),
                validator: request
                    .etag
                    .clone()
                    .or_else(|| request.last_modified.clone()),
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
                    cancellation.cancel();
                    tasks.abort_all();
                    return Ok(DownloadOutcome::Paused);
                }
                Ok(Err(error)) => {
                    cancellation.cancel();
                    tasks.abort_all();
                    return Err(error);
                }
                Err(error) => {
                    cancellation.cancel();
                    tasks.abort_all();
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

/// Whether this chunk layout is responsible for every byte of the file.
///
/// Ascending and gap-free from zero to the end. A layout with a hole in it is not one this
/// engine produces, but it is one a caller could hand over, and a whole-file integrity check
/// made on such a set would be a check of something that was never downloaded.
fn layout_covers_file(chunks: &[ChunkSpec], total: Option<u64>) -> bool {
    let mut ordered: Vec<&ChunkSpec> = chunks.iter().collect();
    ordered.sort_by_key(|chunk| chunk.start);
    let mut reach = 0_u64;
    for chunk in ordered {
        if chunk.start > reach {
            return false;
        }
        // An open-ended chunk runs to the end of whatever the server sends.
        let Some(end) = chunk.end else {
            return true;
        };
        reach = reach.max(end);
    }
    total.is_some_and(|total| reach >= total)
}

struct Worker {
    client: Client,
    limiter: ScopedLimiter,
    /// Shared across every transfer, so one host sees one budget.
    hosts: HostLimits,
    /// Whether this worker's chunk is the whole file.
    covers_whole_file: bool,
    part: PartFile,
    checkpoints: Arc<dyn CheckpointSink>,
    cancellation: CancellationToken,
    url: Url,
    validator: Option<String>,
    require_range: bool,
    headers: Arc<Vec<(String, String)>>,
    method: rd_core::ReplayMethod,
    body: Option<ReplayPayload>,
    approved_origins: Arc<Vec<String>>,
    captured_user_agent: Option<String>,
    /// How this stream's bytes become the file. `None` for every ordinary download.
    transform: Option<Arc<StreamTransform>>,
    /// Finished provider-chunk MACs, shared with every other worker of this file.
    macs: Arc<std::sync::Mutex<std::collections::BTreeMap<usize, [u8; 16]>>>,
}

impl Worker {
    async fn run(self, chunk: ChunkSpec) -> Result<DownloadOutcome, HttpDownloadError> {
        if self.cancellation.is_cancelled() {
            return Ok(DownloadOutcome::Paused);
        }
        let mut builder = match self.method {
            rd_core::ReplayMethod::Get => self.client.get(self.url.clone()),
            rd_core::ReplayMethod::Post => {
                let mut post = self.client.post(self.url.clone());
                if let Some(payload) = &self.body {
                    post = post
                        .header(header::CONTENT_TYPE, payload.content_type.as_str())
                        .body(payload.bytes.clone());
                }
                post
            }
        };
        // Per request rather than on the pooled client: the captured agent usually has to
        // match what the origin saw at capture time, but it must not bleed into unrelated
        // transfers that happen to share a client.
        if let Some(agent) = &self.captured_user_agent
            && let Ok(value) = header::HeaderValue::from_str(agent)
        {
            builder = builder.header(header::USER_AGENT, value);
        }
        for (name, value) in self.headers.iter() {
            if let Ok(name) = header::HeaderName::from_bytes(name.as_bytes())
                && let Ok(value) = header::HeaderValue::from_str(value)
            {
                builder = builder.header(name, value);
            }
        }
        if self.require_range {
            let range = match chunk.end {
                Some(end) => format!("bytes={}-{}", chunk.committed, end.saturating_sub(1)),
                None => format!("bytes={}-", chunk.committed),
            };
            builder = builder.header(header::RANGE, range);
            if let Some(validator) = &self.validator {
                builder = builder.header(header::IF_RANGE, validator);
            }
        }
        // One slot per host, held until this body is drained. Four chunks of one file and
        // several files of one hoster would otherwise open as many connections at once as
        // the queue happens to have work, which is what makes a hoster start answering
        // with throttle pages instead of bytes.
        let _slot = tokio::select! {
            () = self.cancellation.cancelled() => return Ok(DownloadOutcome::Paused),
            slot = self.hosts.acquire(&self.url) => slot,
        };
        let response = builder.send().await.map_err(network_failure)?;
        // Belt and braces. The client's redirect policy already refuses an unapproved hop,
        // but only the final URL proves that no header and no body reached a foreign
        // origin, and a stopped redirect arrives here as an ordinary 3xx response.
        if !self.approved_origins.is_empty() {
            if !crate::redirect::is_approved(&self.approved_origins, response.url()) {
                return Err(crate::redirect::not_allowed(response.url()));
            }
            if response.status().is_redirection() {
                let target = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| response.url().join(value).ok())
                    .unwrap_or_else(|| response.url().clone());
                return Err(crate::redirect::not_allowed(&target));
            }
        }
        if response.status() == StatusCode::PRECONDITION_FAILED
            || response.status() == StatusCode::RANGE_NOT_SATISFIABLE
        {
            return Err(HttpDownloadError::RemoteChanged);
        }
        if !response.status().is_success() {
            return Err(status_failure(response.status(), response.headers()));
        }
        // A success that is not `206` is not automatically a refusal: it may be the whole
        // file, the requested part described by a header instead of the status, a changed
        // remote, or a page that is not the file at all. They are told apart below.
        let mut position = chunk.committed;
        if response.status() == StatusCode::PARTIAL_CONTENT {
            // `206` says a part is coming; only `Content-Range` says *which* part. Trusting
            // the status alone let a server answer `Range: bytes=8388608-` with the head of
            // the file and have it written at offset 8 MiB, checkpointed and completed --
            // silently corrupt, and worst with several chunks in flight.
            self.check_partial_range(&chunk, &response)?;
        } else if self.require_range {
            position = self.unranged_start(&chunk, &response)?;
        }
        let mut checkpoint_position = position;
        let mut checkpoint_time = Instant::now();
        // One accumulator per connection. It starts at the provider chunk this worker's
        // first byte falls in, which the resume plan has already aligned to a boundary.
        let mut walker: Option<MacWalker<'_>> = self
            .transform
            .as_ref()
            .and_then(|transform| transform.mac_walker(position));
        let mut body = response.bytes_stream();
        loop {
            let next = tokio::select! {
                () = self.cancellation.cancelled() => {
                    self.flush(chunk.id, position).await?;
                    return Ok(DownloadOutcome::Paused);
                }
                next = body.next() => next,
            };
            let Some(bytes) = next else { break };
            let bytes = bytes.map_err(network_failure)?;
            if let Some(end) = chunk.end
                && position.saturating_add(bytes.len() as u64) > end
            {
                return Err(HttpDownloadError::RemoteChanged);
            }
            self.limiter.acquire(bytes.len()).await?;
            // The transform runs here and nowhere else: the buffer is already allocated and
            // the offset it belongs at is already known, so decryption costs one pass over
            // bytes that were about to be copied anyway -- no second read, no second file.
            let mut plain = bytes.to_vec();
            if let Some(transform) = &self.transform {
                transform.apply(position, &mut plain);
            }
            let written = plain.len();
            let finished = match walker.as_mut() {
                Some(walker) => walker.feed(position, &plain)?,
                None => Vec::new(),
            };
            self.part
                .write_at(position, plain)
                .await
                .map_err(HttpDownloadError::Local)?;
            position += written as u64;
            // Bytes are on disk and the database does not know it yet. The narrowest and most
            // dangerous window in the whole engine: a resume that trusts the file length here
            // would count bytes nothing ever confirmed.
            rd_core::failpoint!("http.after_chunk_write", || {
                HttpDownloadError::Failure(Failure::new(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    "crash point: http.after_chunk_write".to_owned(),
                ))
            });
            for (index, mac) in finished {
                // Computed but not yet written down. A restart that trusted the byte
                // checkpoint alone here would skip the chunk and never account for it,
                // which is why the resume plan rewinds to the last *recorded* MAC.
                rd_core::failpoint!("http.after_chunk_mac", || {
                    HttpDownloadError::Failure(Failure::new(
                        FailureKind::Transient {
                            retry_after_seconds: None,
                        },
                        "crash point: http.after_chunk_mac".to_owned(),
                    ))
                });
                self.checkpoints
                    .commit_chunk_mac(index as u64, mac)
                    .await
                    .map_err(HttpDownloadError::Internal)?;
                self.macs
                    .lock()
                    .map_err(|_| anyhow::anyhow!("mac state"))?
                    .insert(index, mac);
            }
            if position.saturating_sub(checkpoint_position) >= CHECKPOINT_BYTES
                || checkpoint_time.elapsed() >= CHECKPOINT_INTERVAL
            {
                self.flush(chunk.id, position).await?;
                checkpoint_position = position;
                checkpoint_time = Instant::now();
            }
        }

        if let Some(end) = chunk.end
            && position != end
        {
            return Err(HttpDownloadError::Failure(Failure::new(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                format!("response ended at byte {position}, expected {end}"),
            )));
        }
        self.flush(chunk.id, position).await?;
        Ok(DownloadOutcome::Complete)
    }

    /// Refuses a `206` whose `Content-Range` is not the range this chunk asked for.
    ///
    /// RFC 9110 requires the header on a single-range `206`, so its absence is as much a
    /// reason to refuse as a mismatch: without it there is nothing that says where the body
    /// belongs, and the only alternative is to guess.
    fn check_partial_range(
        &self,
        chunk: &ChunkSpec,
        response: &reqwest::Response,
    ) -> Result<(), HttpDownloadError> {
        let start = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(crate::probe::parse_content_range_start);
        match start {
            Some(start) if start == chunk.committed => Ok(()),
            _ => Err(HttpDownloadError::RangeIgnored),
        }
    }

    /// Byte offset the body of a non-`206` success starts at, or why it cannot be used.
    ///
    /// RFC 9110 gives a server several legitimate answers to a conditional range request
    /// and the status code alone tells apart none of them. Treating every non-`206` as a
    /// refusal ended perfectly good downloads: a small file that arrives complete, and the
    /// `200` a server *must* send once an `If-Range` validator no longer matches, were both
    /// reported as "server ignored a required range request".
    fn unranged_start(
        &self,
        chunk: &ChunkSpec,
        response: &reqwest::Response,
    ) -> Result<u64, HttpDownloadError> {
        // The header decides before the status does: a server that answers `200` while
        // describing the range it sends is serving that range.
        if let Some(start) = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(crate::probe::parse_content_range_start)
        {
            return if start == chunk.committed {
                Ok(start)
            } else {
                Err(HttpDownloadError::RangeIgnored)
            };
        }
        // Neither a range nor a file: a throttle notice or a landing page. Worth another
        // attempt in a minute rather than a dead download -- the same judgement the probe
        // already makes, on the same headers.
        let content_type = header_text(response, &header::CONTENT_TYPE);
        if !crate::probe::looks_downloadable(
            header_text(response, &header::CONTENT_DISPOSITION).as_deref(),
            content_type.as_deref(),
            response.content_length(),
        ) {
            return Err(not_a_file(content_type));
        }
        // Bytes are already on disk and the server is sending the entity from its first
        // byte. `If-Range` went out with the request, so this is how RFC 9110 spells "your
        // validator is stale" -- and the partial file must not be continued with it.
        if chunk.committed > chunk.start {
            return Err(HttpDownloadError::RemoteChanged);
        }
        // A complete body is usable exactly when this chunk is the complete file.
        if self.covers_whole_file {
            Ok(chunk.start)
        } else {
            Err(HttpDownloadError::RangeIgnored)
        }
    }

    async fn flush(&self, chunk_id: ChunkId, position: u64) -> Result<(), HttpDownloadError> {
        self.part
            .sync_data()
            .await
            .map_err(HttpDownloadError::Local)?;
        // Durable on disk, not yet recorded. A restart here must resume from the *older*
        // checkpoint and rewrite the tail — never assume the sync implies the commit.
        rd_core::failpoint!("http.after_part_sync", || {
            HttpDownloadError::Failure(Failure::new(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "crash point: http.after_part_sync".to_owned(),
            ))
        });
        self.checkpoints.commit(chunk_id, position).await?;
        // Recorded. A restart must resume at exactly this offset, re-fetching nothing.
        rd_core::failpoint!("http.after_db_checkpoint", || {
            HttpDownloadError::Failure(Failure::new(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "crash point: http.after_db_checkpoint".to_owned(),
            ))
        });
        Ok(())
    }
}

fn header_text(response: &reqwest::Response, name: &header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// A response that carries a page instead of the file.
///
/// Transient on purpose: a hoster's "please wait" notice is gone a minute later, and the
/// user already knows this wording from the online check.
fn not_a_file(content_type: Option<String>) -> HttpDownloadError {
    let content_type = content_type.unwrap_or_else(|| "unknown".to_owned());
    let message = format!("The server returned {content_type} instead of the requested part");
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        "download.not_a_file",
        message,
    )
    .with_param("content_type", content_type)
    .into()
}

pub(crate) fn network_failure(error: reqwest::Error) -> HttpDownloadError {
    let category = if error.is_connect() {
        FailureKind::Offline
    } else {
        FailureKind::Transient {
            retry_after_seconds: None,
        }
    };
    // `reqwest::Error`'s `Display` appends " for url (...)" with the fully expanded URL,
    // which for a presigned CDN link carries the signature in its query string. The message
    // is persisted in `downloads.last_error_json` and broadcast on SSE, so strip the URL and
    // redact whatever the remaining text still quotes.
    let message = rd_core::redact_text(&error.without_url().to_string());
    Failure::new(category, message).into()
}

pub(crate) fn status_failure(status: StatusCode, headers: &header::HeaderMap) -> HttpDownloadError {
    let retry_after_seconds = headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let category = match status.as_u16() {
        401 => FailureKind::AuthRequired,
        403 => FailureKind::AccountInvalid,
        404 | 410 => FailureKind::Permanent,
        429 => FailureKind::RateLimited {
            retry_after_seconds,
        },
        500..=599 => FailureKind::Transient {
            retry_after_seconds,
        },
        _ => FailureKind::Permanent,
    };
    Failure::new(category, format!("HTTP {status}")).into()
}

#[cfg(test)]
mod tests {
    use std::{
        net::SocketAddr,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use anyhow::Result;
    use async_trait::async_trait;
    use axum::{
        Router,
        body::Body,
        extract::State,
        http::{HeaderMap, HeaderValue},
        response::{IntoResponse, Response},
        routing::get,
    };
    use rd_core::{ChunkId, FailureKind};
    use reqwest::{StatusCode, header};
    use tokio_util::sync::CancellationToken;

    use super::{
        CheckpointSink, DownloadEngine, DownloadOutcome, DownloadRequest, HttpDownloadError,
        status_failure,
    };
    use rd_limits::ScopedLimiter;

    use crate::{ChunkSpec, hostlimit::HostLimits};

    struct NoopCheckpoint;

    async fn serve(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        address
    }

    /// A single whole-file chunk, the shape a file below the chunk size is planned as.
    fn whole_file(total: u64, committed: u64) -> Vec<ChunkSpec> {
        vec![ChunkSpec {
            id: ChunkId::new(),
            start: 0,
            end: Some(total),
            committed,
        }]
    }

    /// A ranged request carrying a validator, as the worker always sends one.
    fn ranged_request(
        address: SocketAddr,
        part_path: std::path::PathBuf,
        total: u64,
        chunks: Vec<ChunkSpec>,
    ) -> DownloadRequest {
        DownloadRequest {
            url: format!("http://{address}/file")
                .parse()
                .expect("fixture URL"),
            part_path,
            total_bytes: Some(total),
            etag: Some("\"fixture\"".to_owned()),
            last_modified: None,
            use_ranges: true,
            chunks,
            headers: Vec::new(),
            method: rd_core::ReplayMethod::Get,
            body: None,
            approved_origins: Arc::new(Vec::new()),
            captured_user_agent: None,
            transform: None,
        }
    }

    /// Answers with the complete body and status 200, as a hoster that does not implement
    /// ranges does.
    ///
    /// Built rather than assembled from a tuple, because a response part appends to the
    /// content type axum derives from the body instead of replacing it, and this fixture
    /// exists precisely to control that header.
    fn full_body(payload: &'static [u8], content_type: &'static str) -> Response {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(payload.to_vec()))
            .expect("fixture response")
    }

    async fn ranged_fixture(headers: HeaderMap) -> Response {
        const PAYLOAD: &[u8] = b"parallel-range-payload";
        let range = headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("bytes="))
            .and_then(|value| value.split_once('-'));
        let Some((start, end)) = range else {
            return (StatusCode::OK, PAYLOAD).into_response();
        };
        let start = start.parse::<usize>().expect("range start");
        let end = end.parse::<usize>().expect("range end");
        let mut response_headers = HeaderMap::new();
        response_headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{}", PAYLOAD.len()))
                .expect("content range"),
        );
        (
            StatusCode::PARTIAL_CONTENT,
            response_headers,
            PAYLOAD[start..=end].to_vec(),
        )
            .into_response()
    }

    #[async_trait]
    impl CheckpointSink for NoopCheckpoint {
        async fn commit(&self, _chunk_id: ChunkId, _committed_offset: u64) -> Result<()> {
            Ok(())
        }
    }

    /// RFC 9110 requires a `200` once an `If-Range` validator no longer matches, so the
    /// full body is the server saying "this is a different file now" -- not that it ignored
    /// the range. Either way the bytes already on disk must survive untouched.
    #[tokio::test]
    async fn a_full_response_to_resume_reports_a_changed_remote_and_keeps_the_part_file() {
        let address = serve(Router::new().route(
            "/file",
            get(|| async { full_body(b"replacement", "application/octet-stream") }),
        ))
        .await;

        let directory = tempfile::tempdir().expect("temporary directory");
        let part_path = directory.path().join("payload.part");
        tokio::fs::write(&part_path, b"safe")
            .await
            .expect("write existing checkpoint");
        let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());
        let result = engine
            .download(
                ranged_request(address, part_path.clone(), 4, whole_file(4, 2)),
                Arc::new(NoopCheckpoint),
                CancellationToken::new(),
            )
            .await;

        assert!(matches!(result, Err(HttpDownloadError::RemoteChanged)));
        assert_eq!(
            tokio::fs::read(part_path).await.expect("read part file"),
            b"safe"
        );
    }

    /// The reported defect: a small file is planned as one chunk, the probe saw
    /// `Accept-Ranges`, and the hoster answers the ranged `GET` with the whole file and a
    /// plain `200`. That is a complete download, and used to be a permanent failure.
    #[tokio::test]
    async fn a_full_body_on_a_whole_file_chunk_is_written_from_the_start() {
        const PAYLOAD: &[u8] = b"whole-file-payload";
        let address = serve(Router::new().route(
            "/file",
            get(|| async { full_body(PAYLOAD, "application/octet-stream") }),
        ))
        .await;
        let directory = tempfile::tempdir().expect("temporary directory");
        let part_path = directory.path().join("whole.part");
        let total = PAYLOAD.len() as u64;
        let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

        let outcome = engine
            .download(
                ranged_request(address, part_path.clone(), total, whole_file(total, 0)),
                Arc::new(NoopCheckpoint),
                CancellationToken::new(),
            )
            .await
            .expect("a complete body is a complete download");

        assert_eq!(outcome, DownloadOutcome::Complete);
        assert_eq!(
            tokio::fs::read(part_path).await.expect("read part file"),
            PAYLOAD
        );
    }

    /// A throttle notice or landing page carries a `200` too. It is not the file, but it is
    /// also not permanent: the same link works minutes later, so it has to be retryable.
    #[tokio::test]
    async fn a_page_instead_of_the_payload_is_retryable_and_translatable() {
        const PAGE: &[u8] = b"<html><body>please wait 30 minutes</body></html>";
        let address = serve(Router::new().route(
            "/file",
            get(|| async { full_body(PAGE, "text/html; charset=utf-8") }),
        ))
        .await;
        let directory = tempfile::tempdir().expect("temporary directory");
        let part_path = directory.path().join("page.part");
        let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

        let result = engine
            .download(
                ranged_request(address, part_path.clone(), 4096, whole_file(4096, 0)),
                Arc::new(NoopCheckpoint),
                CancellationToken::new(),
            )
            .await;

        let Err(HttpDownloadError::Failure(failure)) = result else {
            panic!("a served page must be a classified failure");
        };
        assert_eq!(
            failure.category,
            FailureKind::Transient {
                retry_after_seconds: None
            }
        );
        assert_eq!(failure.code.as_deref(), Some("download.not_a_file"));
        assert_eq!(
            failure.params.get("content_type").map(String::as_str),
            Some("text/html; charset=utf-8")
        );
        // Nothing of the page may reach the part file, which is preallocated and therefore
        // all zeroes until a byte of payload is written.
        let written = tokio::fs::read(part_path).await.expect("part file");
        assert!(
            written.iter().all(|byte| *byte == 0),
            "the served page was written to the part file"
        );
    }

    /// `Content-Range` is the header that says what a body contains. A server that answers
    /// `200` while describing the requested part is serving that part, and the resume
    /// continues at the committed offset instead of being thrown away.
    #[tokio::test]
    async fn a_described_range_is_accepted_even_with_status_200() {
        const PAYLOAD: &[u8] = b"content-range-payload";
        const COMMITTED: usize = 8;
        let address = serve(Router::new().route(
            "/file",
            get(|| async {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(
                        header::CONTENT_RANGE,
                        format!("bytes {COMMITTED}-{}/{}", PAYLOAD.len() - 1, PAYLOAD.len()),
                    )
                    .body(Body::from(PAYLOAD[COMMITTED..].to_vec()))
                    .expect("fixture response")
            }),
        ))
        .await;
        let directory = tempfile::tempdir().expect("temporary directory");
        let part_path = directory.path().join("described.part");
        tokio::fs::write(&part_path, &PAYLOAD[..COMMITTED])
            .await
            .expect("write existing checkpoint");
        let total = PAYLOAD.len() as u64;
        let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

        let outcome = engine
            .download(
                ranged_request(
                    address,
                    part_path.clone(),
                    total,
                    whole_file(total, COMMITTED as u64),
                ),
                Arc::new(NoopCheckpoint),
                CancellationToken::new(),
            )
            .await
            .expect("a described range completes the file");

        assert_eq!(outcome, DownloadOutcome::Complete);
        assert_eq!(
            tokio::fs::read(part_path).await.expect("read part file"),
            PAYLOAD
        );
    }

    /// Nothing used to bound how many connections one host saw: four chunks of a file, and
    /// as many files as the queue allowed, all at once.
    #[tokio::test]
    async fn the_host_limit_bounds_the_connections_one_host_sees() {
        const PAYLOAD: &[u8] = b"parallel-range-payload";
        #[derive(Default)]
        struct Concurrency {
            open: AtomicUsize,
            peak: AtomicUsize,
        }
        async fn counted(State(seen): State<Arc<Concurrency>>, headers: HeaderMap) -> Response {
            let open = seen.open.fetch_add(1, Ordering::SeqCst) + 1;
            seen.peak.fetch_max(open, Ordering::SeqCst);
            // Long enough that unlimited chunks would demonstrably overlap.
            tokio::time::sleep(Duration::from_millis(150)).await;
            seen.open.fetch_sub(1, Ordering::SeqCst);
            ranged_fixture(headers).await
        }

        let seen = Arc::new(Concurrency::default());
        let address = serve(
            Router::new()
                .route("/file", get(counted))
                .with_state(Arc::clone(&seen)),
        )
        .await;
        let directory = tempfile::tempdir().expect("temporary directory");
        let part_path = directory.path().join("limited.part");
        let total = PAYLOAD.len() as u64;
        let bounds = [0_u64, 6, 12, 17, total];
        let chunks = bounds
            .windows(2)
            .map(|pair| ChunkSpec {
                id: ChunkId::new(),
                start: pair[0],
                end: Some(pair[1]),
                committed: pair[0],
            })
            .collect::<Vec<_>>();
        let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited())
            .with_host_limits(HostLimits::new(2));

        let outcome = engine
            .download(
                ranged_request(address, part_path.clone(), total, chunks),
                Arc::new(NoopCheckpoint),
                CancellationToken::new(),
            )
            .await
            .expect("a limited download still completes");

        assert_eq!(outcome, DownloadOutcome::Complete);
        assert_eq!(
            tokio::fs::read(part_path).await.expect("read part file"),
            PAYLOAD
        );
        assert!(
            seen.peak.load(Ordering::SeqCst) <= 2,
            "the host saw {} simultaneous connections, the limit was 2",
            seen.peak.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn parallel_ranges_reconstruct_the_exact_file() {
        const PAYLOAD: &[u8] = b"parallel-range-payload";
        let app = Router::new().route("/file", get(ranged_fixture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        let directory = tempfile::tempdir().expect("temporary directory");
        let part_path = directory.path().join("parallel.part");
        let split = 9_u64;
        let total = PAYLOAD.len() as u64;
        let engine = DownloadEngine::new(reqwest::Client::new(), ScopedLimiter::unlimited());

        let result = engine
            .download(
                DownloadRequest {
                    url: format!("http://{address}/file")
                        .parse()
                        .expect("fixture URL"),
                    part_path: part_path.clone(),
                    total_bytes: Some(total),
                    etag: Some("\"fixture\"".to_owned()),
                    last_modified: None,
                    use_ranges: true,
                    chunks: vec![
                        ChunkSpec {
                            id: ChunkId::new(),
                            start: 0,
                            end: Some(split),
                            committed: 0,
                        },
                        ChunkSpec {
                            id: ChunkId::new(),
                            start: split,
                            end: Some(total),
                            committed: split,
                        },
                    ],
                    headers: Vec::new(),
                    method: rd_core::ReplayMethod::Get,
                    body: None,
                    approved_origins: Arc::new(Vec::new()),
                    captured_user_agent: None,
                    transform: None,
                },
                Arc::new(NoopCheckpoint),
                CancellationToken::new(),
            )
            .await
            .expect("parallel download");

        assert_eq!(result, super::DownloadOutcome::Complete);
        assert_eq!(
            tokio::fs::read(part_path).await.expect("part file"),
            PAYLOAD
        );
    }

    #[test]
    fn retry_after_and_auth_statuses_keep_their_taxonomy() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::RETRY_AFTER, "17".parse().expect("header"));
        let HttpDownloadError::Failure(rate_limited) =
            status_failure(StatusCode::TOO_MANY_REQUESTS, &headers)
        else {
            panic!("expected classified failure");
        };
        assert_eq!(
            rate_limited.category,
            FailureKind::RateLimited {
                retry_after_seconds: Some(17)
            }
        );

        let HttpDownloadError::Failure(auth) =
            status_failure(StatusCode::UNAUTHORIZED, &header::HeaderMap::new())
        else {
            panic!("expected classified failure");
        };
        assert_eq!(auth.category, FailureKind::AuthRequired);
    }
}
