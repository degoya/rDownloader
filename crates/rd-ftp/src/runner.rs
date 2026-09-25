//! FTP/FTPS transport as a scheduler runner: one remote file per queue entry.

use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{
    DownloadFile, DownloadKind, DownloadPackage, Failure, FailureKind, RemoteTarget, StorageRootId,
};
use rd_files::StorageRoot;
use rd_scheduler::{ExternalRunner, RunLimits, RunOutcome};
use rd_transfer_file::{Labels, Resume, Staging};
use tokio_util::sync::CancellationToken;

use crate::{
    FtpService,
    error::{self, classify},
};

/// The stable code and the wording this crate puts on the shared transfer's failures.
const LABELS: Labels = Labels {
    length_mismatch: error::CONNECT_FAILED,
    length_mismatch_message: "The FTP transfer did not deliver the expected number of bytes",
    stalled: "the FTP server stopped sending data",
    open_part: "open the partial FTP download",
    flush_part: "flush the FTP download",
};

/// Downloads one FTP file per queue row.
pub struct FtpRunner {
    service: FtpService,
}

impl FtpRunner {
    #[must_use]
    pub const fn new(service: FtpService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl ExternalRunner for FtpRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Ftp
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
                "The download address is not a valid FTP link",
            )));
        };
        let credential = match self
            .service
            .credential_for(file.remote_credential_id, &target)
            .await?
        {
            Ok(credential) => credential,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };

        if package.destination.is_empty() {
            anyhow::bail!("ftp package has no destination directory");
        }
        let root = StorageRoot::create(
            StorageRootId::new(),
            "download destination".to_owned(),
            PathBuf::from(&package.destination),
        )
        .await?;
        let destination = root.path().to_path_buf();
        tokio::fs::create_dir_all(&destination).await?;
        let part_path = rd_files::part_path(&root, file.id).await?;

        let mut connection = match self.service.connect(&credential).await? {
            Ok(connection) => connection,
            Err(error) => return Ok(RunOutcome::Failed(classify(&error))),
        };
        let outcome = self
            .execute(
                &mut connection,
                Transfer {
                    file,
                    target: &target,
                    root: &root,
                    part_path: &part_path,
                    cancellation,
                    bandwidth: &limits.bandwidth,
                },
            )
            .await;
        // The control connection is closed either way; a dropped socket leaves the server
        // holding a login slot until its own idle timeout expires.
        let _ = connection.quit().await;
        outcome
    }
}

/// Everything one transfer needs beyond the connection itself.
struct Transfer<'a> {
    file: &'a DownloadFile,
    target: &'a RemoteTarget,
    root: &'a StorageRoot,
    part_path: &'a std::path::Path,
    cancellation: CancellationToken,
    bandwidth: &'a rd_limits::ScopedLimiter,
}

impl FtpRunner {
    async fn execute(
        &self,
        connection: &mut crate::client::Connection,
        transfer: Transfer<'_>,
    ) -> Result<RunOutcome> {
        let Transfer {
            file,
            target,
            root,
            part_path,
            cancellation,
            bandwidth,
        } = transfer;
        let path = &target.path;
        let size = match connection.size(path).await {
            Ok(size) => size as u64,
            Err(error) => return Ok(RunOutcome::Failed(classify(&error))),
        };
        let modified = connection
            .modified_at(path)
            .await
            .ok()
            .map(|naive| naive.and_utc().to_rfc3339());

        let staging =
            Staging::open(self.service.database(), file, root, part_path, size, LABELS).await;
        match staging.plan_resume(modified).await? {
            // Size and timestamp are all FTP gives us to recognise the file again.
            Resume::Refused => return Ok(RunOutcome::Failed(error::file_changed())),
            Resume::Complete => return staging.promote().await,
            Resume::Continue => {
                if let Err(error) = connection.resume_from(staging.committed() as usize).await {
                    return Ok(RunOutcome::Failed(match &error {
                        // A refused REST is not a transient server problem; it means this
                        // transfer can never be continued and needs a deliberate restart.
                        suppaftp::FtpError::UnexpectedResponse(_) => error::resume_unsupported(),
                        other => classify(other),
                    }));
                }
            }
            Resume::Fresh => {}
        }

        let mut sink = staging.open_part().await?;
        let end = match connection
            .retrieve(
                path,
                &staging,
                &mut sink,
                bandwidth,
                &cancellation,
                self.service.timeout(),
            )
            .await?
        {
            Ok(end) => end,
            Err(error) => return Ok(RunOutcome::Failed(classify(&error))),
        };
        staging.finish(sink, end).await
    }
}
