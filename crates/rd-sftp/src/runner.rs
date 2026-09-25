//! SFTP transport as a scheduler runner: one remote file per queue entry.

use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{
    DownloadFile, DownloadKind, DownloadPackage, Failure, FailureKind, RemoteTarget, StorageRootId,
};
use rd_files::StorageRoot;
use rd_scheduler::{ExternalRunner, RunLimits, RunOutcome};
use rd_transfer_file::{Labels, Resume, Staging};
use russh_sftp::client::SftpSession;
use tokio::io::AsyncSeekExt;
use tokio_util::sync::CancellationToken;

use crate::{SftpService, error, listing};

/// The stable code and the wording this crate puts on the shared transfer's failures.
const LABELS: Labels = Labels {
    length_mismatch: error::CONNECT_FAILED,
    length_mismatch_message: "The SFTP transfer did not deliver the expected number of bytes",
    stalled: "the SFTP server stopped sending data",
    open_part: "open the partial SFTP download",
    flush_part: "flush the SFTP download",
};

/// Downloads one SFTP file per queue row.
pub struct SftpRunner {
    service: SftpService,
}

impl SftpRunner {
    #[must_use]
    pub const fn new(service: SftpService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl ExternalRunner for SftpRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Sftp
    }

    fn slot_capacity(&self) -> usize {
        self.service.max_parallel()
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: RunLimits,
    ) -> Result<RunOutcome> {
        let Some(target) = RemoteTarget::parse(&file.source) else {
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Permanent,
                error::CONNECT_FAILED,
                "The download address is not a valid SFTP link",
            )));
        };
        if package.destination.is_empty() {
            anyhow::bail!("sftp package has no destination directory");
        }
        let root = StorageRoot::create(
            StorageRootId::new(),
            "download destination".to_owned(),
            PathBuf::from(&package.destination),
        )
        .await?;
        tokio::fs::create_dir_all(root.path()).await?;
        let part_path = rd_files::part_path(&root, file.id).await?;

        let connection = match self
            .service
            .connect(file.remote_credential_id, &target)
            .await?
        {
            Ok(connection) => connection,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        self.execute(
            &connection.sftp,
            Transfer {
                file,
                target: &target,
                root: &root,
                part_path: &part_path,
                cancellation,
                bandwidth: &limits.bandwidth,
            },
        )
        .await
    }
}

/// Everything one transfer needs beyond the session itself.
struct Transfer<'a> {
    file: &'a DownloadFile,
    target: &'a RemoteTarget,
    root: &'a StorageRoot,
    part_path: &'a std::path::Path,
    cancellation: CancellationToken,
    bandwidth: &'a rd_limits::ScopedLimiter,
}

impl SftpRunner {
    async fn execute(&self, sftp: &SftpSession, transfer: Transfer<'_>) -> Result<RunOutcome> {
        let Transfer {
            file,
            target,
            root,
            part_path,
            cancellation,
            bandwidth,
        } = transfer;
        let path = &target.path;
        let metadata = match sftp.metadata(path.clone()).await {
            Ok(metadata) => metadata,
            Err(error) => return Ok(RunOutcome::Failed(error::classify_sftp(&error))),
        };
        if metadata.is_dir() {
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Permanent,
                error::PATH_NOT_FOUND,
                "The remote path is a directory, not a file",
            )));
        }
        let Some(size) = listing::size_of(&metadata).map(rd_core::ByteCount::get) else {
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Permanent,
                error::PATH_NOT_FOUND,
                "The SFTP server did not report the file size",
            )));
        };
        let modified = listing::modified_at(&metadata).map(|value| value.to_rfc3339());

        let staging =
            Staging::open(self.service.database(), file, root, part_path, size, LABELS).await;
        // Size and modification time are the only validators SFTP offers.
        let resume = match staging.plan_resume(modified).await? {
            Resume::Refused => return Ok(RunOutcome::Failed(error::file_changed())),
            Resume::Complete => return staging.promote().await,
            other => other,
        };

        let mut remote = match sftp.open(path.clone()).await {
            Ok(remote) => remote,
            Err(error) => return Ok(RunOutcome::Failed(error::classify_sftp(&error))),
        };
        // Unlike FTP's REST, seeking is part of the protocol and never refused, so a resume
        // needs no capability check.
        if matches!(resume, Resume::Continue)
            && remote
                .seek(std::io::SeekFrom::Start(staging.committed()))
                .await
                .is_err()
        {
            return Ok(RunOutcome::Failed(error::file_changed()));
        }
        let mut sink = staging.open_part().await?;

        let end = staging
            .stream(
                &mut remote,
                &mut sink,
                bandwidth,
                &cancellation,
                self.service.timeout(),
            )
            .await?;
        staging.finish(sink, end).await
    }
}
