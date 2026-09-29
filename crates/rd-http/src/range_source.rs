//! Mirrors the chunk engine reaches through another protocol (RD-150-03).
//!
//! A Metalink document names FTP and SFTP mirrors beside HTTP ones. The chunk engine speaks
//! HTTP itself; for the others it asks a [`RangeSource`] — the FTP and SFTP runners each
//! provide one — for the file's size and for a reader positioned at a chunk's offset, and
//! writes what that reader yields into the part file with the same checkpoints, pacing and
//! cancellation as an HTTP chunk. A chunk is one connection: opened at its offset, read to its
//! end, closed.
//!
//! Such a mirror is an address a stranger's document named, like every other source of the
//! set. Every connection a range source opens is held to the download's [`AddressPolicy`]
//! when the socket is opened: the name is resolved once, every address is checked, and the
//! connection goes to exactly those ([`crate::connect_addresses`]).

use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use rd_core::{ChunkId, Failure, FailureKind, RemoteTarget};
use rd_files::PartFile;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::{
    AddressPolicy, CheckpointSink, ChunkSpec, DownloadEngine, DownloadOutcome, HttpDownloadError,
    engine::{CHECKPOINT_BYTES, CHECKPOINT_INTERVAL},
};

/// Bytes asked of a mirror's reader at a time.
const READ_BYTES: usize = 64 * 1024;
/// How long a mirror may send nothing before the chunk moves on to another source.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// What a range source hands back: the file from the requested offset on. Dropping it closes
/// the connection it reads from.
pub type RangeReader = Box<dyn AsyncRead + Send + Unpin>;

/// A mirror protocol the chunk engine does not speak itself.
///
/// `policy` is `None` only where a test drives the source against a fixture on this machine;
/// the queue always passes the download's rule. A refusal is a [`Failure`] coded
/// [`rd_core::CODE_INTERNAL_ADDRESS`], which isolates the mirror.
#[async_trait]
pub trait RangeSource: Send + Sync {
    /// The size of the file `target` names.
    async fn size(
        &self,
        target: &RemoteTarget,
        policy: Option<&AddressPolicy>,
    ) -> Result<u64, Failure>;

    /// A reader yielding the file `target` names from byte `offset` on.
    async fn open_at(
        &self,
        target: &RemoteTarget,
        offset: u64,
        policy: Option<&AddressPolicy>,
    ) -> Result<RangeReader, Failure>;
}

/// A mirror fetched through a [`RangeSource`], with the address rule it keeps to.
#[derive(Clone)]
pub struct RangeTransport {
    pub source: Arc<dyn RangeSource>,
    pub target: RemoteTarget,
    pub policy: Option<AddressPolicy>,
}

impl fmt::Debug for RangeTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RangeTransport")
            .field("protocol", &self.target.protocol)
            .field("host", &self.target.host)
            .field("port", &self.target.port)
            .field("guarded", &self.policy.is_some())
            .finish_non_exhaustive()
    }
}

impl DownloadEngine {
    /// Fetches one bounded chunk through a range source: from its confirmed offset to its end.
    ///
    /// What arrived is synced and checkpointed before any way out, a failure included, so the
    /// next source continues from the last byte this one delivered.
    pub(crate) async fn fetch_via(
        &self,
        transport: &RangeTransport,
        part: PartFile,
        checkpoints: Arc<dyn CheckpointSink>,
        cancellation: CancellationToken,
        chunk: ChunkSpec,
    ) -> Result<DownloadOutcome, HttpDownloadError> {
        let end = chunk.end.ok_or_else(|| {
            HttpDownloadError::Internal(anyhow::anyhow!(
                "a chunk fetched from a mirror needs an end"
            ))
        })?;
        let mut position = chunk.committed;
        if position >= end {
            return Ok(DownloadOutcome::Complete);
        }
        let opened = tokio::select! {
            () = cancellation.cancelled() => return Ok(DownloadOutcome::Paused),
            opened = transport.source.open_at(
                &transport.target,
                position,
                transport.policy.as_ref(),
            ) => opened,
        };
        let mut reader = opened.map_err(HttpDownloadError::Failure)?;
        let mut buffer = vec![0_u8; READ_BYTES];
        let mut checkpointed = (position, Instant::now());
        while position < end {
            let wanted =
                usize::try_from(end - position).map_or(READ_BYTES, |left| left.min(READ_BYTES));
            let read = tokio::select! {
                () = cancellation.cancelled() => {
                    commit(&part, checkpoints.as_ref(), chunk.id, position).await?;
                    return Ok(DownloadOutcome::Paused);
                }
                read = tokio::time::timeout(READ_TIMEOUT, reader.read(&mut buffer[..wanted])) => read,
            };
            let read = match read {
                Ok(Ok(0)) => break,
                Ok(Ok(read)) => read,
                Ok(Err(error)) => {
                    commit(&part, checkpoints.as_ref(), chunk.id, position).await?;
                    return Err(network_failed(&error.to_string()));
                }
                Err(_) => {
                    commit(&part, checkpoints.as_ref(), chunk.id, position).await?;
                    return Err(network_failed("the mirror stopped sending data"));
                }
            };
            self.limiter().acquire(read).await?;
            part.write_at(position, buffer[..read].to_vec())
                .await
                .map_err(HttpDownloadError::Local)?;
            position += read as u64;
            if position - checkpointed.0 >= CHECKPOINT_BYTES
                || checkpointed.1.elapsed() >= CHECKPOINT_INTERVAL
            {
                commit(&part, checkpoints.as_ref(), chunk.id, position).await?;
                checkpointed = (position, Instant::now());
            }
        }
        commit(&part, checkpoints.as_ref(), chunk.id, position).await?;
        if position != end {
            return Err(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "download.response_truncated",
                format!("the mirror ended at byte {position}, expected {end}"),
            )
            .with_param("position", position)
            .with_param("expected", end)
            .into());
        }
        Ok(DownloadOutcome::Complete)
    }
}

/// Makes the bytes durable, then records them.
async fn commit(
    part: &PartFile,
    checkpoints: &dyn CheckpointSink,
    chunk_id: ChunkId,
    position: u64,
) -> Result<(), HttpDownloadError> {
    part.sync_data().await.map_err(HttpDownloadError::Local)?;
    checkpoints
        .commit(chunk_id, position)
        .await
        .map_err(HttpDownloadError::Internal)
}

fn network_failed(detail: &str) -> HttpDownloadError {
    let detail = rd_core::redact_text(detail);
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        "download.network_failed",
        detail.clone(),
    )
    .with_param("detail", detail)
    .into()
}

#[cfg(test)]
#[path = "range_source_tests.rs"]
mod tests;
