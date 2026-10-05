//! The body of a response written into the part file, and the checkpoints that make it
//! durable.

use std::time::Instant;

use futures_util::StreamExt;
use rd_core::{ChunkId, Failure, FailureKind};

use crate::{ChunkSpec, transform::MacWalker};

use super::{
    CHECKPOINT_BYTES, CHECKPOINT_INTERVAL, DownloadOutcome, HttpDownloadError,
    failure::network_failure, worker::Worker,
};

impl Worker {
    /// Streams the body from `position` to the end of the chunk, checkpointing on the way.
    pub(super) async fn write_body(
        &self,
        chunk: &ChunkSpec,
        response: reqwest::Response,
        mut position: u64,
    ) -> Result<DownloadOutcome, HttpDownloadError> {
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
            position = self
                .write_bytes(chunk, position, &bytes, walker.as_mut())
                .await?;
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
            return Err(HttpDownloadError::Failure(
                Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    "download.response_truncated",
                    format!("response ended at byte {position}, expected {end}"),
                )
                .with_param("position", position)
                .with_param("expected", end),
            ));
        }
        self.flush(chunk.id, position).await?;
        Ok(DownloadOutcome::Complete)
    }

    /// Writes one received buffer at `position` and returns the position after it.
    async fn write_bytes(
        &self,
        chunk: &ChunkSpec,
        position: u64,
        bytes: &[u8],
        walker: Option<&mut MacWalker<'_>>,
    ) -> Result<u64, HttpDownloadError> {
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
        let finished = match walker {
            Some(walker) => walker.feed(position, &plain)?,
            None => Vec::new(),
        };
        self.part
            .write_at(position, plain)
            .await
            .map_err(HttpDownloadError::Local)?;
        let position = position + written as u64;
        // Bytes are on disk and the database does not know it yet. The narrowest and most
        // dangerous window in the whole engine: a resume that trusts the file length here
        // would count bytes nothing ever confirmed.
        rd_core::failpoint!("http.after_chunk_write", || {
            HttpDownloadError::Failure(
                Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    "download.crash_point",
                    "crash point: http.after_chunk_write",
                )
                .with_param("point", "http.after_chunk_write"),
            )
        });
        self.record_macs(finished).await?;
        Ok(position)
    }

    /// Records the provider-chunk MACs one buffer finished.
    async fn record_macs(&self, finished: Vec<(usize, [u8; 16])>) -> Result<(), HttpDownloadError> {
        for (index, mac) in finished {
            // Computed but not yet written down. A restart that trusted the byte
            // checkpoint alone here would skip the chunk and never account for it,
            // which is why the resume plan rewinds to the last *recorded* MAC.
            rd_core::failpoint!("http.after_chunk_mac", || {
                HttpDownloadError::Failure(
                    Failure::coded(
                        FailureKind::Transient {
                            retry_after_seconds: None,
                        },
                        "download.crash_point",
                        "crash point: http.after_chunk_mac",
                    )
                    .with_param("point", "http.after_chunk_mac"),
                )
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
        Ok(())
    }

    async fn flush(&self, chunk_id: ChunkId, position: u64) -> Result<(), HttpDownloadError> {
        self.part
            .sync_data()
            .await
            .map_err(HttpDownloadError::Local)?;
        // Durable on disk, not yet recorded. A restart here must resume from the *older*
        // checkpoint and rewrite the tail — never assume the sync implies the commit.
        rd_core::failpoint!("http.after_part_sync", || {
            HttpDownloadError::Failure(
                Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    "download.crash_point",
                    "crash point: http.after_part_sync",
                )
                .with_param("point", "http.after_part_sync"),
            )
        });
        self.checkpoints.commit(chunk_id, position).await?;
        // Recorded. A restart must resume at exactly this offset, re-fetching nothing.
        rd_core::failpoint!("http.after_db_checkpoint", || {
            HttpDownloadError::Failure(
                Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    "download.crash_point",
                    "crash point: http.after_db_checkpoint",
                )
                .with_param("point", "http.after_db_checkpoint"),
            )
        });
        Ok(())
    }
}
