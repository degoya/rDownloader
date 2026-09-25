//! Promotion of a finished transfer: checksum verification, collision-free naming and the
//! move out of the staging directory into the package destination.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rd_core::{ChecksumAlgorithm, DownloadFile, DownloadState, ExpectedChecksum, StorageRootId};
use rd_files::{StorageRoot, collision_free_path, compute_checksum};

use crate::SchedulerHandle;

/// Canonical destination of the file's package as stored right now.
pub(crate) async fn current_destination(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
) -> Result<Option<PathBuf>> {
    let packages = scheduler.database.list_packages().await?;
    let Some(package) = packages
        .into_iter()
        .find(|package| package.id == file.package_id)
    else {
        return Ok(None);
    };
    let root = StorageRoot::create(
        StorageRootId::new(),
        "download destination".to_owned(),
        PathBuf::from(&package.destination),
    )
    .await?;
    Ok(Some(root.path().to_path_buf()))
}

pub(crate) async fn prepare_final_path(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    destination: &Path,
) -> Result<PathBuf> {
    let current = destination.join(&file.file_name);
    if !current.exists() {
        return Ok(current);
    }
    let collision = collision_free_path(destination, &file.file_name);
    let name = collision
        .file_name()
        .and_then(|value| value.to_str())
        .context("collision filename is not Unicode")?
        .to_owned();
    scheduler
        .database
        .set_download_file_name(file.id, name)
        .await?;
    Ok(collision)
}

pub(crate) async fn verify_part_and_promote(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    part_path: &Path,
    final_path: &Path,
) -> Result<()> {
    let computed = compute_configured_checksum(scheduler, file, part_path).await?;
    verify_expected(file, computed.as_ref())?;
    tokio::fs::rename(part_path, final_path).await?;
    // The instant the recovery matrix is about: the payload is already in its final place and
    // the row still says the transfer is running. Stopping here is the worse half of the two
    // phases — the `.part` the resume would look for is gone — so a case can prove the next
    // pass adopts the file that is there instead of fetching it a second time.
    rd_core::failpoint!("scheduler.before_promote", || anyhow::anyhow!(
        "crash point: scheduler.before_promote"
    ));
    let final_name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .context("final filename is not Unicode")?
        .to_owned();
    scheduler
        .database
        .complete_download(file.id, final_name, computed)
        .await?;
    Ok(())
}

pub(crate) async fn verify_and_complete(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    final_path: &Path,
) -> Result<()> {
    scheduler
        .database
        .transition_download(file.id, DownloadState::Verifying)
        .await?;
    let computed = compute_configured_checksum(scheduler, file, final_path).await?;
    verify_expected(file, computed.as_ref())?;
    scheduler
        .database
        .complete_download(file.id, file.file_name.clone(), computed)
        .await?;
    Ok(())
}

/// Adopts a payload that is already lying in its final place, finished.
///
/// The restart half of the promote window (`scheduler.before_promote`): the rename out of
/// staging happened, `complete_download` did not, and `recover_interrupted` put the row back
/// into the queue. Without this the `.part` the resume looks for is gone, the whole file is
/// fetched a second time and `collision_free_path` files the copy beside the original as
/// `name (1).ext` — two full-sized files, neither of them wrong, and no way for the queue to
/// say which one it meant.
///
/// Returns whether the file was adopted; `false` means the caller downloads as usual.
pub(crate) async fn adopt_existing_final(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    destination: &Path,
    staging: &Path,
    part_path: &Path,
    total_bytes: Option<u64>,
) -> Result<bool> {
    // A part file that is still there means the transfer stopped before the rename, which is
    // the ordinary resume and not this case at all.
    if part_path.exists() {
        return Ok(false);
    }
    let final_path = destination.join(&file.file_name);
    let Ok(metadata) = tokio::fs::metadata(&final_path).await else {
        return Ok(false);
    };
    if !is_finished_payload(metadata.len(), file.committed_bytes.get(), total_bytes) {
        return Ok(false);
    }
    // The row is `Resolving` here and the state machine has no edge from there to `Verifying`.
    // The step it is missing is the transfer itself, and for a file that is already complete
    // that step is a no-op.
    scheduler
        .database
        .transition_download(file.id, DownloadState::Downloading)
        .await?;
    verify_and_complete(scheduler, file, &final_path).await?;
    remove_if_empty(staging).await;
    Ok(true)
}

/// Whether a file of `on_disk` bytes in the final place is this download, finished.
///
/// Its length is the whole evidence, so the test is deliberately strict: the download must
/// have confirmed bytes, the file must be exactly that long, and a host that stated a length
/// must have been reached. The earlier check compared the confirmed count against the stated
/// length alone and never looked at the file, so a host that sends no `Content-Length` —
/// chunked delivery, which is common — adopted nothing and re-fetched what it already had.
fn is_finished_payload(on_disk: u64, committed: u64, total_bytes: Option<u64>) -> bool {
    committed > 0 && on_disk == committed && total_bytes.is_none_or(|total| total == committed)
}

/// The two halves of the promote window, reachable from the recovery-matrix case.
///
/// That window lives inside the worker, which needs a live host to be driven to it. Rather
/// than stand one up for a case that is about a database write, the two steps are exported
/// behind the feature the case needs anyway — a release build compiles neither of them.
#[cfg(feature = "failpoints")]
impl SchedulerHandle {
    /// Promotes a verified `.part` into `final_path` and records the completion.
    pub async fn promote_finished_part(
        &self,
        file: &DownloadFile,
        part_path: &Path,
        final_path: &Path,
    ) -> Result<()> {
        verify_part_and_promote(self, file, part_path, final_path).await
    }

    /// The restart decision: adopt a finished file already in `destination`, or say no.
    pub async fn adopt_finished_file(
        &self,
        file: &DownloadFile,
        destination: &Path,
        staging: &Path,
        part_path: &Path,
        total_bytes: Option<u64>,
    ) -> Result<bool> {
        adopt_existing_final(self, file, destination, staging, part_path, total_bytes).await
    }
}

/// Removes a directory only when it is empty — a staging directory another package file
/// still uses, or a destination that still holds data, is left alone.
pub(crate) async fn remove_if_empty(directory: &Path) {
    match tokio::fs::remove_dir(directory).await {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => tracing::warn!(
            path = %directory.display(),
            %error,
            "empty download directory was not removed"
        ),
    }
}

async fn compute_configured_checksum(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    path: &Path,
) -> Result<Option<ExpectedChecksum>> {
    let algorithm = file.expected_checksum.as_ref().map(|value| value.algorithm);
    let algorithm = algorithm.or_else(|| {
        scheduler
            .generate_sha256()
            .then_some(ChecksumAlgorithm::Sha256)
    });
    match algorithm {
        Some(algorithm) => {
            let computed = compute_checksum(path, algorithm).await?;
            Ok(Some(ExpectedChecksum {
                algorithm: computed.algorithm,
                value: computed.value,
            }))
        }
        None => Ok(None),
    }
}

/// The downloaded bytes do not match the checksum the source stated for them.
pub(crate) const CHECKSUM_MISMATCH_CODE: &str = "download.checksum_mismatch";

fn verify_expected(file: &DownloadFile, computed: Option<&ExpectedChecksum>) -> Result<()> {
    if let (Some(expected), Some(computed)) = (&file.expected_checksum, computed)
        && !expected.value.eq_ignore_ascii_case(&computed.value)
    {
        // A typed failure rather than a bare message: the bytes are wrong, which is the
        // hoster's doing and not this machine's, and the caller has to be able to tell the
        // two apart before it decides whether another mirror is worth trying (RD-110-20).
        return Err(anyhow::Error::new(rd_core::Failure::coded(
            rd_core::FailureKind::Permanent,
            CHECKSUM_MISMATCH_CODE,
            format!(
                "checksum mismatch: expected {}, got {}",
                expected.value, computed.value
            ),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_finished_payload, remove_if_empty};

    #[test]
    fn a_file_is_only_adopted_when_its_length_proves_it_is_the_finished_payload() {
        assert!(is_finished_payload(1024, 1024, Some(1024)));
        // No stated length: the confirmed count is all there is, and it still has to match.
        assert!(is_finished_payload(1024, 1024, None));
        // Short on disk, truncated, or a different file that happens to share the name.
        assert!(!is_finished_payload(512, 1024, Some(1024)));
        assert!(!is_finished_payload(2048, 1024, Some(1024)));
        // Nothing was ever confirmed, so whatever is lying there is not ours.
        assert!(!is_finished_payload(1024, 0, Some(1024)));
        // Confirmed bytes that never reached the stated length are a partial transfer.
        assert!(!is_finished_payload(512, 512, Some(1024)));
    }

    #[tokio::test]
    async fn removes_staging_only_after_its_last_part_file_is_gone() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let staging = temporary.path().join(".rdownloader");
        tokio::fs::create_dir(&staging)
            .await
            .expect("staging directory");
        let part = staging.join("download.part");
        tokio::fs::write(&part, b"partial")
            .await
            .expect("part file");

        remove_if_empty(&staging).await;
        assert!(staging.is_dir(), "non-empty staging directory must remain");

        tokio::fs::remove_file(part)
            .await
            .expect("remove part file");
        remove_if_empty(&staging).await;
        assert!(!staging.exists(), "empty staging directory must be removed");
    }
}
