//! The half of a remote file transfer that has nothing to do with the protocol.
//!
//! FTP and SFTP differ only in how the bytes are asked for. Everything around that — the
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

/// How much is read from the remote source at a time.
const CHUNK_BYTES: usize = 64 * 1024;

/// How many bytes are written before the queue row is updated again.
///
/// One database write per megabyte rather than one per read: the checkpoint that matters for
/// a resume is the file on disk, and the row only has to be roughly current.
const PROGRESS_INTERVAL_BYTES: u64 = 1024 * 1024;

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
        if self.committed == 0 {
            self.database
                .prepare_transfer(self.file.id, Some(self.size), None, modified, Vec::new())
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
        // More on disk than the server says the whole file holds means the two disagree
        // about what was downloaded, whatever the validators claim.
        if size_changed || time_changed || self.committed > self.size {
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
            if written.saturating_sub(last_reported) >= PROGRESS_INTERVAL_BYTES {
                last_reported = written;
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
        if matches!(end, TransferEnd::Stopped) {
            return Ok(RunOutcome::Stopped);
        }
        let on_disk = rd_files::existing_bytes(self.part_path).await;
        self.database
            .set_download_progress(self.file.id, on_disk, Some(self.size))
            .await?;
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
mod tests {
    use rd_core::{DownloadFile, DownloadKind, DownloadState, StorageRootId};
    use rd_db::{Database, NewDownload, NewPackage};
    use rd_files::StorageRoot;
    use rd_scheduler::RunOutcome;
    use tokio::io::AsyncWriteExt;

    use super::{Labels, Staging, TransferEnd};

    /// The labels are the caller's; any pair of constants exercises the shared code.
    const LABELS: Labels = Labels {
        length_mismatch: "test.length_mismatch",
        length_mismatch_message: "The transfer did not deliver the expected number of bytes",
        stalled: "the test server stopped sending data",
        open_part: "open the partial test download",
        flush_part: "flush the test download",
    };

    struct Fixture {
        _directory: tempfile::TempDir,
        database: Database,
        root: StorageRoot,
        file: DownloadFile,
        part_path: std::path::PathBuf,
    }

    impl Fixture {
        async fn start() -> Self {
            let directory = tempfile::tempdir().expect("tempdir");
            let database = Database::open(directory.path().join("staging.sqlite3"))
                .await
                .expect("database");
            let destination = directory.path().join("downloads");
            tokio::fs::create_dir_all(&destination).await.expect("dir");
            let root = StorageRoot::create(
                StorageRootId::new(),
                "download destination".to_owned(),
                destination,
            )
            .await
            .expect("root");
            let package_id = rd_core::PackageId::new();
            database
                .create_package(NewPackage {
                    id: package_id,
                    name: "staging".to_owned(),
                    destination: root.path().to_string_lossy().into_owned(),
                    category_id: None,
                    priority: rd_core::DownloadPriority::Normal,
                    postprocess_level: None,
                    script: None,
                    enrichment: Vec::new(),
                })
                .await
                .expect("package");
            let file = database
                .create_download(NewDownload {
                    id: rd_core::DownloadId::new(),
                    package_id,
                    source: "ftp://127.0.0.1/pub/movie.bin".parse().expect("url"),
                    file_name: "movie.bin".to_owned(),
                    total_bytes: None,
                    expected_checksum: None,
                    account_id: None,
                    proxy_profile_id: None,
                    auth_profile: rd_core::AuthProfileSelection::Auto,
                    initial_state: DownloadState::Queued,
                    kind: DownloadKind::Ftp,
                    media: None,
                    remote_credential_id: None,
                    mirror_group: None,
                    replay: None,
                    enrichment: Vec::new(),
                    secret_fragment: None,
                })
                .await
                .expect("download");
            let part_path = rd_files::part_path(&root, file.id)
                .await
                .expect("part path");
            Self {
                _directory: directory,
                database,
                root,
                file,
                part_path,
            }
        }

        async fn staging(&self, size: u64) -> Staging<'_> {
            Staging::open(
                &self.database,
                &self.file,
                &self.root,
                &self.part_path,
                size,
                LABELS,
            )
            .await
        }

        fn final_path(&self) -> std::path::PathBuf {
            self.root.path().join("movie.bin")
        }
    }

    /// Writes `bytes` into the staging file and hands back the open handle, the way a
    /// finished read loop leaves it.
    async fn part_with(fixture: &Fixture, bytes: &[u8]) -> tokio::fs::File {
        let staging = fixture.staging(0).await;
        let mut sink = staging.open_part().await.expect("open part");
        sink.write_all(bytes).await.expect("write");
        sink
    }

    #[tokio::test]
    async fn a_short_delivery_is_refused_and_the_partial_file_is_kept() {
        let fixture = Fixture::start().await;
        let sink = part_with(&fixture, &[7u8; 900]).await;

        let staging = fixture.staging(1000).await;
        let outcome = staging
            .finish(sink, TransferEnd::Complete)
            .await
            .expect("finish");

        match outcome {
            RunOutcome::Failed(failure) => {
                assert_eq!(failure.code.as_deref(), Some(LABELS.length_mismatch));
            }
            other => panic!("a short delivery must be refused, got {other:?}"),
        }
        // Refusing must not throw away what was downloaded, and must not publish it.
        assert!(fixture.part_path.exists());
        assert!(!fixture.final_path().exists());
    }

    #[tokio::test]
    async fn a_long_delivery_is_refused_rather_than_promoted() {
        let fixture = Fixture::start().await;
        // The defect this pins: a server that acknowledges a resume offset and then streams
        // from byte zero appends a second copy behind the part already on disk. Accepting
        // anything that merely reaches the announced size publishes that as the payload.
        let sink = part_with(&fixture, &[7u8; 1400]).await;

        let staging = fixture.staging(1000).await;
        let outcome = staging
            .finish(sink, TransferEnd::Complete)
            .await
            .expect("finish");

        match outcome {
            RunOutcome::Failed(failure) => {
                assert_eq!(failure.code.as_deref(), Some(LABELS.length_mismatch));
            }
            other => panic!("a long delivery must be refused, got {other:?}"),
        }
        assert!(fixture.part_path.exists());
        assert!(!fixture.final_path().exists());
    }

    #[tokio::test]
    async fn a_complete_delivery_is_synced_before_it_is_promoted() {
        let fixture = Fixture::start().await;
        let payload = vec![7u8; 1000];
        // Deliberately not flushed here: `finish` owns the sync, and the rename that
        // publishes the file must not happen before it. Renaming first would publish a
        // file whose tail is still in a buffer this process has not handed to the kernel.
        let sink = part_with(&fixture, &payload).await;

        let staging = fixture.staging(1000).await;
        let outcome = staging
            .finish(sink, TransferEnd::Complete)
            .await
            .expect("finish");

        match outcome {
            RunOutcome::Completed { final_name } => assert_eq!(final_name, "movie.bin"),
            other => panic!("a complete delivery must be promoted, got {other:?}"),
        }
        let published = tokio::fs::read(fixture.final_path()).await.expect("final");
        assert_eq!(published, payload);
        // The staging file must not survive a completed transfer.
        assert!(!fixture.part_path.exists());
    }
}
