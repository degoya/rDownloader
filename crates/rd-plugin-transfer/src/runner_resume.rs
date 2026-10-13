//! Where a plugin transfer continues: the staging bytes no build owns are dropped, the remote
//! file's validators are compared with the ones recorded at the start, and a backend that
//! cannot continue starts over. Split from `runner.rs` so neither file outgrows the 500-line
//! rule (RD-1280-08).

use std::path::Path;

use anyhow::{Context, Result};
use rd_core::{DownloadFile, Failure, FailureKind};
use rd_plugin_host::RemoteFile;

use super::{PluginTransferRunner, logged};

/// Removes what a staging file holds while no pin names the build that wrote it (RD-1120-18).
///
/// Since the pin comes before the first byte, such bytes are from no build anybody can name: a
/// transfer that kept them would continue another build's file the moment a different version
/// had been installed in between.
pub(super) async fn discard_unowned_bytes(part_path: &Path) -> Result<()> {
    match tokio::fs::remove_file(part_path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("discard staging bytes no backend version owns"),
    }
}

impl PluginTransferRunner {
    /// Where the transfer continues in the staging file: its length, when the remote file is
    /// still the one it was started on and the backend can continue; otherwise from the start.
    pub(super) async fn resume_offset(
        &self,
        file: &DownloadFile,
        remote: &RemoteFile,
        part_path: &Path,
    ) -> std::result::Result<u64, Failure> {
        // The staging file and its length come from `rd-files`, not from
        // `rd_transfer_file::Staging`, which owns the same two things for FTP and SFTP. That
        // type is built around a host-side loop that reads an `AsyncRead` into a file it
        // opened itself and syncs it before the rename; a plugin transfer has no source in the
        // host at all. The guest writes through the `PartFile` it was handed, the throttle and
        // the progress write live in `TransferState`, and the staging file has to exist before
        // the probe can run — so before any size is known, where `Staging` wants an exact one.
        let committed = rd_files::existing_bytes(part_path).await;
        if let Some(failure) = self.validate_resume(file, remote, committed).await {
            return Err(failure);
        }
        // A backend that cannot continue starts over rather than writing new bytes on top of
        // old ones, which is the one way a resume can corrupt without ever erroring.
        Ok(if remote.resumable { committed } else { 0 })
    }

    async fn validate_resume(
        &self,
        file: &DownloadFile,
        remote: &rd_plugin_host::RemoteFile,
        committed: u64,
    ) -> Option<Failure> {
        if committed == 0 {
            logged(
                self.database
                    .prepare_transfer(
                        file.id,
                        remote.size,
                        None,
                        remote.last_modified.clone(),
                        Vec::new(),
                    )
                    .await,
                "record a new plugin transfer's validators",
            );
            return None;
        }
        let stored = self.database.load_transfer(file.id).await.ok()?;
        // Both validators are compared verbatim: a server that moved either has a different
        // file behind the same URL, and continuing would splice two of them together.
        let changed = stored.total_bytes != remote.size
            || (stored.last_modified.is_some() && stored.last_modified != remote.last_modified)
            || remote.size.is_some_and(|size| committed > size);
        changed.then(|| {
            Failure::coded(
                FailureKind::Permanent,
                "plugin.file_changed",
                "The remote file changed since this download started",
            )
        })
    }
}
