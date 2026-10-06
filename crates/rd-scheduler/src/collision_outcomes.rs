//! How a collision ends without a new file: skipped, or the existing file adopted as this
//! download — and the content index a finished file is put into.

use std::path::Path;

use anyhow::Result;
use rd_core::{
    ChecksumAlgorithm, DownloadFile, DownloadId, DownloadState, ExpectedChecksum, Failure,
    FailureKind,
};
use rd_files::compute_checksum;

use super::INDEX_ALGORITHM;
use crate::{SchedulerHandle, failures::record_error};

/// Ends the download without writing: the existing file stays exactly as it was.
pub(super) async fn skip(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    part_path: &Path,
) -> Result<()> {
    discard_part(part_path).await;
    scheduler.database.clear_collision_prompt(file.id).await?;
    record_error(
        scheduler,
        file,
        Failure::coded(
            FailureKind::Permanent,
            rd_core::CODE_COLLISION_SKIPPED,
            "the file name is taken and the collision policy is skip".to_owned(),
        ),
    )
    .await
}

/// Whether the existing file is this download before it is fetched: a stated checksum is
/// proof either way, a different stated size is proof of a difference, and anything else is
/// no proof at all (`None`).
pub(super) async fn compare_before(
    file: &DownloadFile,
    existing: &Path,
    existing_bytes: u64,
    total_bytes: Option<u64>,
) -> Result<Option<bool>> {
    if let Some(expected) = &file.expected_checksum {
        let computed = compute_checksum(existing, expected.algorithm).await?;
        return Ok(Some(computed.value.eq_ignore_ascii_case(&expected.value)));
    }
    Ok(total_bytes
        .filter(|total| *total != existing_bytes)
        .map(|_| false))
}

/// Whether the verified part file and the existing file hold the same bytes. The part's own
/// digest is reused when there is one; otherwise both sides are hashed with SHA-256.
pub(super) async fn same_content(
    part_path: &Path,
    existing: &Path,
    computed: Option<&ExpectedChecksum>,
) -> Result<bool> {
    let part_bytes = tokio::fs::metadata(part_path).await?.len();
    if tokio::fs::metadata(existing).await?.len() != part_bytes {
        return Ok(false);
    }
    let (algorithm, part_digest) = match computed {
        Some(computed) => (computed.algorithm, computed.value.clone()),
        None => (
            ChecksumAlgorithm::Sha256,
            compute_checksum(part_path, ChecksumAlgorithm::Sha256)
                .await?
                .value,
        ),
    };
    let existing_digest = compute_checksum(existing, algorithm).await?.value;
    Ok(existing_digest.eq_ignore_ascii_case(&part_digest))
}

/// The existing file is this download: finish on it instead of fetching it again.
pub(super) async fn adopt_before(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    existing: &Path,
    part_path: &Path,
) -> Result<()> {
    discard_part(part_path).await;
    // `Resolving` has no edge to `Verifying`; the transfer it skips is a no-op for a file that
    // is already complete, exactly as in `finish::adopt_existing_final`.
    scheduler
        .database
        .transition_download(file.id, DownloadState::Downloading)
        .await?;
    // Indexes the file too, like every other completion.
    crate::finish::verify_and_complete(scheduler, file, existing).await?;
    scheduler.database.clear_collision_prompt(file.id).await?;
    Ok(())
}

/// The verified part file equals the existing one: drop the part, keep the file.
pub(super) async fn adopt_after(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    existing: &Path,
    name: &str,
    part_path: &Path,
    computed: Option<&ExpectedChecksum>,
) -> Result<()> {
    discard_part(part_path).await;
    scheduler
        .database
        .complete_download(file.id, name.to_owned(), computed.cloned())
        .await?;
    index_finished(scheduler, file.id, existing, computed).await;
    scheduler.database.clear_collision_prompt(file.id).await?;
    Ok(())
}

async fn discard_part(part_path: &Path) {
    if let Err(error) = tokio::fs::remove_file(part_path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %part_path.display(), %error, "the part file was not discarded");
    }
}

/// Puts a finished file into the content index when its SHA-256 is known.
///
/// Best effort: the download is complete either way, and a file the index misses is found by
/// the next check, which backfills from the rows (`check_content_index`).
pub(crate) async fn index_finished(
    scheduler: &SchedulerHandle,
    id: DownloadId,
    path: &Path,
    computed: Option<&ExpectedChecksum>,
) {
    let Some(computed) = computed.filter(|value| value.algorithm == ChecksumAlgorithm::Sha256)
    else {
        return;
    };
    let result = async {
        let size = tokio::fs::metadata(path).await?.len();
        scheduler
            .database
            .index_content(
                id,
                INDEX_ALGORITHM.to_owned(),
                computed.value.clone(),
                size,
                path.to_string_lossy().into_owned(),
            )
            .await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(download_id = %id, %error, "the finished file was not indexed");
    }
}
