//! The half of a remote file transfer that has nothing to do with the protocol.
//!
//! FTP, SFTP and object storage differ only in how the bytes are asked for. Everything around that — the
//! `.rdownloader/<id>.part` staging file, the size-and-timestamp check that decides whether
//! a partial download may be continued, the throttled progress write, the guard on the
//! delivered length and the sync that has to precede the rename which publishes the file —
//! was a copy in each crate. Two corrections had to be made twice because of that, which is
//! the reason this shape now lives in one place and both runners drive it.
//!
//! The staging path itself and the length already on disk are *not* here: they are the one
//! part of this that the plugin transfer needs too, and that runner does not fit `Staging`, so
//! `rd_files::part_path` and `rd_files::existing_bytes` own them for all three callers.
//!
//! It is a crate of its own rather than part of `rd-files`, where shared file handling
//! otherwise belongs, because it needs `rd-db`, `rd-scheduler` and `rd-limits` and all three
//! depend on `rd-files` themselves — putting it there is a dependency cycle Cargo refuses.
//! Nor does it belong to either runner: `rd-sftp` depending on `rd-ftp` would compile, but it
//! says the wrong thing about what these two crates are to each other.

#![warn(unreachable_pub)]

mod remote;

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rd_core::{DownloadFile, Failure, FailureKind};
use rd_db::Database;
use rd_files::StorageRoot;
use rd_limits::ScopedLimiter;
use rd_scheduler::RunOutcome;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub use remote::{
    DirectoryLister, ListedEntry, LiveRemoteSettings, SharedRemoteSettings, credential_for, join,
    walk,
};

/// How much is read from the remote source at a time.
const CHUNK_BYTES: usize = 64 * 1024;

/// How many bytes are written before the staging file is synced and the queue row updated.
///
/// The checkpoint that matters for a resume is the file on disk, so it is synced before the
/// row is told: the row never claims bytes the disk may not hold after a power cut (audit
/// 1.9.1, TR-17). The same rhythm as the HTTP engine's checkpoints, 8 MiB or two seconds,
/// so a fast line does not pay for a sync per megabyte and a slow one still shows progress.
const CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;

/// The longest the row waits for a checkpoint while bytes are arriving.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);

/// What ended the read loop.
pub enum TransferEnd {
    /// The source closed after the payload.
    Complete,
    /// The queue asked the transfer to stop; the partial file is left in place.
    Stopped,
}

/// What the validators say about a partial file that is already on disk.
pub enum Resume {
    /// Nothing was downloaded yet; the validators for this attempt have been recorded.
    Fresh,
    /// The partial file is still valid and the transfer continues behind it.
    Continue,
    /// The partial file already holds the whole payload.
    Complete,
    /// The remote file moved under the partial download. The caller raises its own
    /// `file_changed` failure, because every stable code the queue reports names its
    /// protocol and the client translates that code rather than any text.
    Refused,
}

/// The stable codes and wording one protocol puts on the shared failures.
///
/// Passed in as the caller's own constants for the same reason [`Resume::Refused`] carries
/// no failure: the code is part of each crate's published error vocabulary.
pub struct Labels {
    /// Stable code for a delivery whose length is not the size the server announced.
    pub length_mismatch: &'static str,
    /// Queue message for that mismatch.
    pub length_mismatch_message: &'static str,
    /// Message for a source that stops sending without ending the transfer.
    pub stalled: &'static str,
    /// `anyhow` context for opening the staging file.
    pub open_part: &'static str,
    /// `anyhow` context for the sync that has to precede the publishing rename.
    pub flush_part: &'static str,
}

/// One remote file's staging area: the partial file, its validators and the rename that
/// publishes it.
pub struct Staging<'a> {
    database: &'a Database,
    file: &'a DownloadFile,
    root: &'a StorageRoot,
    part_path: &'a Path,
    labels: Labels,
    committed: u64,
    size: u64,
}

impl<'a> Staging<'a> {
    /// Reads how much of `part_path` is already on disk and records the announced `size`.
    pub async fn open(
        database: &'a Database,
        file: &'a DownloadFile,
        root: &'a StorageRoot,
        part_path: &'a Path,
        size: u64,
        labels: Labels,
    ) -> Self {
        let committed = rd_files::existing_bytes(part_path).await;
        Self {
            database,
            file,
            root,
            part_path,
            labels,
            committed,
            size,
        }
    }

    /// Bytes already on disk when this attempt started.
    #[must_use]
    pub const fn committed(&self) -> u64 {
        self.committed
    }

    /// Decides whether the partial file may be continued, and records the validators when
    /// there is nothing to continue from.
    ///
    /// Size and modification time are all that either protocol offers to recognise the file
    /// again. When one of them moved, the partial file is kept but the transfer is refused:
    /// continuing would write the new file's bytes behind the old file's, and nothing
    /// downstream would notice.
    pub async fn plan_resume(&self, modified: Option<String>) -> Result<Resume> {
        self.plan_resume_validated(None, modified).await
    }

    /// [`Self::plan_resume`] for a protocol that also names a version of the file: an object
    /// store's `ETag`. A different one refuses the resume exactly like a different size.
    ///
    /// The value is compared as an opaque string and nothing else. What an `ETag` is made of
    /// differs by service, upload path and encryption, so reading more into it — a digest of
    /// the content, say — would be a guess.
    pub async fn plan_resume_validated(
        &self,
        etag: Option<String>,
        modified: Option<String>,
    ) -> Result<Resume> {
        if self.committed == 0 {
            self.database
                .prepare_transfer(self.file.id, Some(self.size), etag, modified, Vec::new())
                .await?;
            return Ok(Resume::Fresh);
        }
        let stored = self.database.load_transfer(self.file.id).await?;
        let size_changed = stored.total_bytes.is_some_and(|old| old != self.size);
        let time_changed = stored
            .last_modified
            .as_ref()
            .zip(modified.as_ref())
            .is_some_and(|(old, new)| old != new);
        let version_changed = stored
            .etag
            .as_ref()
            .zip(etag.as_ref())
            .is_some_and(|(old, new)| old != new);
        // More on disk than the server says the whole file holds means the two disagree
        // about what was downloaded, whatever the validators claim.
        if size_changed || time_changed || version_changed || self.committed > self.size {
            return Ok(Resume::Refused);
        }
        if self.committed == self.size {
            return Ok(Resume::Complete);
        }
        Ok(Resume::Continue)
    }

    /// Opens the staging file for appending, creating it on the first attempt.
    pub async fn open_part(&self) -> Result<tokio::fs::File> {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.part_path)
            .await
            .context(self.labels.open_part)
    }

    /// Streams `source` into `sink` until it ends or the queue stops the transfer.
    ///
    /// The source must already be positioned at [`Self::committed`]; this function never
    /// seeks, because a protocol that silently ignored the request to resume would
    /// otherwise have its restart written on top of the existing partial file.
    pub async fn stream<R>(
        &self,
        source: &mut R,
        sink: &mut tokio::fs::File,
        bandwidth: &ScopedLimiter,
        cancellation: &CancellationToken,
        read_timeout: Duration,
    ) -> Result<TransferEnd>
    where
        R: AsyncRead + Unpin,
    {
        let mut buffer = vec![0u8; CHUNK_BYTES];
        let mut written = self.committed;
        let mut last_reported = self.committed;
        let mut reported_at = tokio::time::Instant::now();
        loop {
            // Also checked ahead of the select, which picks at random between two ready
            // branches: a transfer that has been stopped should not write another chunk.
            if cancellation.is_cancelled() {
                return Ok(TransferEnd::Stopped);
            }
            let read = tokio::select! {
                () = cancellation.cancelled() => return Ok(TransferEnd::Stopped),
                result = tokio::time::timeout(read_timeout, source.read(&mut buffer)) => match result {
                    Ok(result) => result?,
                    Err(_) => anyhow::bail!("{}", self.labels.stalled),
                },
            };
            if read == 0 {
                return Ok(TransferEnd::Complete);
            }
            // Throttle before writing, so the limit shapes what is pulled from the network
            // rather than only what reaches the disk.
            bandwidth.acquire(read).await?;
            sink.write_all(&buffer[..read]).await?;
            written += read as u64;
            if written.saturating_sub(last_reported) >= CHECKPOINT_BYTES
                || reported_at.elapsed() >= CHECKPOINT_INTERVAL
            {
                sink.sync_data().await.context(self.labels.flush_part)?;
                // Durable on disk, not yet in the row: the next run resumes at the length of
                // the part file, whatever the row says.
                rd_core::failpoint!("transfer_file.before_progress_recorded", || {
                    anyhow::anyhow!("crash point: transfer_file.before_progress_recorded")
                });
                last_reported = written;
                reported_at = tokio::time::Instant::now();
                self.database
                    .set_download_progress(self.file.id, written, Some(self.size))
                    .await?;
            }
        }
    }

    /// Syncs the staging file, then either reports the stop or checks the delivered length
    /// and publishes the file.
    pub async fn finish(&self, sink: tokio::fs::File, end: TransferEnd) -> Result<RunOutcome> {
        // The rename below publishes this file. Without the sync, a crash straight after it
        // leaves a file the queue calls Completed whose tail is still only in page cache.
        sink.sync_all().await.context(self.labels.flush_part)?;
        drop(sink);
        // Recorded on a stop too: the checkpoint cadence leaves the row up to 8 MiB behind the
        // part file, and a paused row showed that lag, and the traffic odometer undercounted
        // it, until the transfer was resumed (re-audit 1.9.1, RA-TR-05).
        let on_disk = rd_files::existing_bytes(self.part_path).await;
        self.database
            .set_download_progress(self.file.id, on_disk, Some(self.size))
            .await?;
        if matches!(end, TransferEnd::Stopped) {
            return Ok(RunOutcome::Stopped);
        }
        if on_disk != self.size {
            // Short *or* long, both mean the bytes on disk are not the file. Short is an
            // early end of the data connection; long is a server that acknowledged the
            // resume offset and then streamed from byte zero anyway, appending a second
            // copy behind the part already there.
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                self.labels.length_mismatch,
                self.labels.length_mismatch_message,
            )));
        }
        self.promote().await
    }

    /// Moves the completed partial file to its final name inside the package folder.
    pub async fn promote(&self) -> Result<RunOutcome> {
        let name = rd_files::sanitize_file_name(&self.file.file_name);
        let final_path = self.root.resolve(Path::new(&name))?;
        if let Some(parent) = final_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(self.part_path, &final_path).await?;
        Ok(RunOutcome::Completed { final_name: name })
    }
}

#[cfg(test)]
mod tests;
