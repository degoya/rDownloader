//! Keeping the content index true, and finishing storage work a stop interrupted
//! (RD-150-01, RD-150-02).
//!
//! The index is written where a file is finished and moved, and removed with its download by
//! the foreign key. What neither can see is the disk changing underneath: a file deleted or
//! moved by hand. [`SchedulerHandle::check_content_index`] is the answer to that, and it is
//! deliberately a check rather than a watcher — run at every start and on request, cheap
//! (one `stat` per entry), and it never forgets an entry because a disk was not mounted: a
//! missing file is *marked* missing and found again when it comes back.

use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use rd_core::{ChecksumAlgorithm, DownloadState};

use crate::{SchedulerHandle, collision::INDEX_ALGORITHM};

/// What one check found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentIndexCheck {
    /// Entries looked at, after the backfill.
    pub checked: usize,
    /// Entries whose file is not where they say, nor where the download's row says.
    pub missing: usize,
    /// Entries that were missing and are found again, at their path or at the row's.
    pub restored: usize,
    /// Finished downloads with a known SHA-256 that had no entry and got one.
    pub backfilled: usize,
}

async fn is_file(path: &std::path::Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

impl SchedulerHandle {
    /// Brings the content index in line with the disk and the queue.
    pub async fn check_content_index(&self) -> Result<ContentIndexCheck> {
        let destinations = self
            .database
            .list_packages()
            .await?
            .into_iter()
            .map(|package| (package.id, PathBuf::from(package.destination)))
            .collect::<HashMap<_, _>>();
        let downloads = self.database.list_downloads().await?;
        let expected_paths = downloads
            .iter()
            .filter_map(|file| {
                destinations
                    .get(&file.package_id)
                    .filter(|destination| !destination.as_os_str().is_empty())
                    .map(|destination| (file.id, destination.join(&file.file_name)))
            })
            .collect::<HashMap<_, _>>();
        let mut report = ContentIndexCheck::default();

        // A finished file whose index write was lost — a stop between the completion and the
        // entry, or a row finished before the index existed — is found through its row.
        let indexed = self
            .database
            .list_content_index()
            .await?
            .into_iter()
            .map(|entry| entry.download_id)
            .collect::<std::collections::HashSet<_>>();
        for file in &downloads {
            let Some(checksum) = file
                .computed_checksum
                .as_ref()
                .filter(|checksum| checksum.algorithm == ChecksumAlgorithm::Sha256)
            else {
                continue;
            };
            if file.state != DownloadState::Completed || indexed.contains(&file.id) {
                continue;
            }
            let Some(path) = expected_paths.get(&file.id) else {
                continue;
            };
            let Ok(metadata) = tokio::fs::metadata(path).await else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            self.database
                .index_content(
                    file.id,
                    INDEX_ALGORITHM.to_owned(),
                    checksum.value.clone(),
                    metadata.len(),
                    path.to_string_lossy().into_owned(),
                )
                .await?;
            report.backfilled += 1;
        }

        let mut changes = Vec::new();
        for entry in self.database.list_content_index().await? {
            report.checked += 1;
            let was_missing = entry.missing_since.is_some();
            if is_file(std::path::Path::new(&entry.path)).await {
                if was_missing {
                    changes.push((entry.download_id, false));
                    report.restored += 1;
                }
                continue;
            }
            // Moved by the queue in a way that did not reach the index: the row knows the way.
            if let Some(expected) = expected_paths.get(&entry.download_id)
                && expected.as_os_str() != entry.path.as_str()
                && is_file(expected).await
            {
                self.database
                    .move_indexed_content(
                        entry.download_id,
                        expected.to_string_lossy().into_owned(),
                    )
                    .await?;
                if was_missing {
                    report.restored += 1;
                }
                continue;
            }
            report.missing += 1;
            if !was_missing {
                changes.push((entry.download_id, true));
            }
        }
        self.database.mark_indexed_content(changes).await?;
        Ok(report)
    }

    /// What a start owes the storage work of the previous run: carry on every category move
    /// that still has data to carry, and check the index. Its history is settled before this,
    /// in `start`, so no row this run writes can be taken for one of the last run's.
    ///
    /// Runs once, in the background, after the queue is up. A move resumes through the same
    /// verified protocol it started with, so whether the stop left the data at the old place or
    /// verified at both, the next pass ends with exactly one copy at the new one.
    pub(crate) async fn recover_storage_work(&self) {
        match self.database.packages_with_outstanding_move().await {
            Ok(packages) => {
                for package_id in packages {
                    if let Err(error) = self.relocate_package(package_id).await {
                        tracing::warn!(
                            %package_id,
                            %error,
                            "an interrupted category move was not completed"
                        );
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "outstanding category moves were not read"),
        }
        match self.check_content_index().await {
            Ok(report) if report.missing > 0 || report.backfilled > 0 => {
                tracing::info!(
                    checked = report.checked,
                    missing = report.missing,
                    backfilled = report.backfilled,
                    "content index checked"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "the content index was not checked"),
        }
    }
}
