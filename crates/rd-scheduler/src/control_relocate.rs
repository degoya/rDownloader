//! Carrying a package's data over to the destination its new category resolved to.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{DownloadFile, DownloadId, NzbImportId, PackageId};
use rd_files::{
    VerifiedMoveError, collision_free_path, move_directory, move_file, place_verified,
    release_source, verified_move_file,
};

use super::is_active;
use crate::SchedulerHandle;

impl SchedulerHandle {
    /// Carries a package's data over to the destination its new category resolved to.
    ///
    /// Only files that are not transferring right now are moved. A running file keeps writing
    /// into the `.part` it already has open; because the destination is read again at promotion
    /// time, it lands in the new folder by itself when it finishes. The former directory is
    /// therefore swept only once nothing of the package is running any more — which is why this
    /// is called both on the category change and after every completed file.
    ///
    /// Nothing to do, and no error, when the package has no outstanding move recorded.
    pub async fn relocate_package(&self, package_id: PackageId) -> Result<()> {
        let _one_move_at_a_time = self.relocations.lock().await;
        let Some(previous) = self
            .database
            .package_previous_destination(package_id)
            .await?
        else {
            return Ok(());
        };
        let Some(package) = self.database.get_package(package_id).await? else {
            return Ok(());
        };
        let from = PathBuf::from(&previous);
        let to = PathBuf::from(&package.destination);
        // The instant the recovery matrix is about: the row already names `to`, and nothing has
        // moved yet. Stopping here is the worst case of the two-phase protocol, so a case can
        // prove the next pass still finds the data under `from` and carries it over.
        rd_core::failpoint!("scheduler.before_package_move", || anyhow::anyhow!(
            "crash point: scheduler.before_package_move"
        ));
        if from == to || package.destination.is_empty() {
            self.database
                .clear_package_previous_destination(package_id)
                .await?;
            return Ok(());
        }
        let import_id = package.nzb_import_id;
        let files = self.database.downloads_for_package(package_id).await?;
        let running = self
            .active
            .lock()
            .await
            .tokens
            .keys()
            .copied()
            .collect::<Vec<_>>();

        let mut outstanding = false;
        for file in &files {
            if is_active(file.state) || running.contains(&file.id) {
                outstanding = true;
                continue;
            }
            if let Err(error) = self.relocate_file(file, &from, &to, import_id).await {
                outstanding = true;
                tracing::warn!(
                    download_id = %file.id,
                    %error,
                    "file was not carried over to the new category directory"
                );
            }
        }
        if outstanding {
            return Ok(());
        }
        // Only the files the database knows about were moved above, and only from the top
        // level. Extracted output has no download row at all — it is found by walking the
        // directory — and archive parts in a subfolder were never descended into, so both
        // stayed behind while the package record claimed to have moved. Carry the rest over,
        // but only out of a directory that belongs to this package alone.
        if owns_directory(&from, &to, &self.config.downloads_directory) {
            carry_over_remaining(&from, &to).await;
        }
        sweep_former_directory(&from, &to, &self.config.downloads_directory, import_id).await;
        self.database
            .clear_package_previous_destination(package_id)
            .await?;
        Ok(())
    }

    /// Moves one file's payload or its incomplete staging file from `from` to `to`.
    async fn relocate_file(
        &self,
        file: &DownloadFile,
        from: &Path,
        to: &Path,
        import_id: Option<NzbImportId>,
    ) -> Result<()> {
        let payload = from.join(&file.file_name);
        if tokio::fs::try_exists(&payload).await? {
            tokio::fs::create_dir_all(to).await?;
            let target = self.move_payload(file, &payload, to).await?;
            if target.file_name() != payload.file_name()
                && let Some(name) = target.file_name().and_then(|value| value.to_str())
            {
                self.database
                    .set_download_file_name(file.id, name.to_owned())
                    .await?;
            }
            self.database
                .move_indexed_content(file.id, target.to_string_lossy().into_owned())
                .await?;
        }
        // The checkpoint is carried over too, and not as an alternative to the payload: a job
        // that has both would otherwise leave its `.part` behind, which both loses the resume
        // point and keeps the old directory from ever being swept.
        let part_name = match (import_id, file.nzb_file_id) {
            (Some(_), Some(nzb_file_id)) => format!("{nzb_file_id}.part"),
            _ => format!("{}.part", file.id),
        };
        let staging = match (import_id, file.nzb_file_id) {
            (Some(import_id), Some(_)) => Some(import_id),
            _ => None,
        };
        let source = staging_directory(from, staging).join(&part_name);
        if !tokio::fs::try_exists(&source).await? {
            return Ok(());
        }
        let target_staging = staging_directory(to, staging);
        tokio::fs::create_dir_all(&target_staging).await?;
        move_file(&source, &target_staging.join(&part_name)).await
    }
}

impl SchedulerHandle {
    /// Whether `path` is the payload another download's row names.
    async fn owned_by_another(&self, path: &Path, this: DownloadId) -> Result<bool> {
        let destinations = self
            .database
            .list_packages()
            .await?
            .into_iter()
            .map(|package| (package.id, PathBuf::from(package.destination)))
            .collect::<std::collections::HashMap<_, _>>();
        Ok(self
            .database
            .list_downloads()
            .await?
            .iter()
            .filter(|other| other.id != this && !other.file_name.is_empty())
            .filter_map(|other| {
                destinations
                    .get(&other.package_id)
                    .map(|destination| destination.join(&other.file_name))
            })
            .any(|payload| payload == path))
    }

    /// Carries one finished payload into `to` with a verified move, recorded in the storage
    /// history (RD-150-02). Answers where it landed.
    ///
    /// The name it had is kept when that name is free in `to`, or when the file there is this
    /// very payload — what a move that stopped after its copy leaves behind. A file of the same
    /// name with other bytes belongs to somebody else and is never overwritten by a move; the
    /// payload is filed beside it instead.
    async fn move_payload(
        &self,
        file: &DownloadFile,
        payload: &Path,
        to: &Path,
    ) -> Result<PathBuf> {
        let mut target = to.join(&file.file_name);
        // A file there that another download owns is that download's, however identical its
        // bytes: taking it for this move's own copy would leave two rows naming one file and
        // remove this payload. It is filed beside it instead, as before.
        if self.owned_by_another(&target, file.id).await? {
            target = collision_free_path(to, &file.file_name);
        }
        let operation = self
            .database
            .start_storage_operation(rd_db::NewStorageOperation {
                kind: rd_core::StorageOperationKind::Move,
                package_id: Some(file.package_id),
                download_id: Some(file.id),
                source_path: payload.to_string_lossy().into_owned(),
                target_path: target.to_string_lossy().into_owned(),
                size_bytes: None,
            })
            .await?;
        let placed = match place_verified(payload, &target).await {
            Err(VerifiedMoveError::TargetTaken(_)) => {
                target = collision_free_path(to, &file.file_name);
                place_verified(payload, &target).await
            }
            other => other,
        };
        let result = async {
            let placed = placed?;
            // The instant between a verified copy and the removal of the original: both are
            // there, identical. The next pass must finish the move with one copy, not file a
            // second one as `name (1)` and not lose either.
            rd_core::failpoint!("scheduler.before_move_source_removed", || {
                VerifiedMoveError::Io {
                    from: payload.to_path_buf(),
                    to: target.clone(),
                    source: std::io::Error::other(
                        "crash point: scheduler.before_move_source_removed",
                    ),
                }
            });
            release_source(payload, &placed).await?;
            Ok::<_, VerifiedMoveError>(placed)
        }
        .await;
        let outcome = match &result {
            Ok(placed) => rd_db::StorageOperationOutcome {
                target_path: Some(target.to_string_lossy().into_owned()),
                ..rd_db::StorageOperationOutcome::completed(
                    Some(placed.size_bytes),
                    placed.digest.clone(),
                )
            },
            Err(error) => rd_db::StorageOperationOutcome::failed(
                error.code(),
                rd_core::error_with_causes(error),
            ),
        };
        if let Err(error) = self
            .database
            .finish_storage_operation(operation, outcome)
            .await
        {
            tracing::warn!(%error, download_id = %file.id, "the move was not recorded in the history");
        }
        result?;
        Ok(target)
    }
}

/// The staging directory a file of this kind writes its `.part` into, below `base`.
fn staging_directory(base: &Path, import_id: Option<NzbImportId>) -> PathBuf {
    match import_id {
        Some(import_id) => base.join(format!(".rdownloader-{import_id}")),
        None => base.join(".rdownloader"),
    }
}

/// Whether `from` holds this package's data alone, so its whole content may be carried over.
///
/// The two exceptions are the ones the sweep below also spares: the service download directory,
/// and the category directory itself (the shape of a row written before packages had their own
/// folder). Both are shared with other packages, whose files must never be dragged along.
fn owns_directory(from: &Path, to: &Path, downloads_directory: &Path) -> bool {
    from != downloads_directory && to.parent() != Some(from)
}

/// Moves whatever is still sitting in the former package directory into the new one.
///
/// Best-effort: a leftover that cannot be moved is logged and skipped, which leaves the old
/// directory standing rather than losing the file. Staging directories are skipped because
/// `relocate_file` already carried their checkpoints over.
async fn carry_over_remaining(from: &Path, to: &Path) {
    // The listing is taken in full before anything moves: moving entries out of a directory
    // that is still being iterated makes the reader skip the ones behind them.
    let mut leftovers = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(from).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with(".rdownloader") {
            continue;
        }
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        leftovers.push((name, entry.path(), file_type.is_dir()));
    }
    if leftovers.is_empty() {
        return;
    }
    if tokio::fs::create_dir_all(to).await.is_err() {
        return;
    }
    for (name, path, is_directory) in leftovers {
        let target = collision_free_path(to, &name);
        let result = if is_directory {
            move_directory(&path, &target).await
        } else {
            verified_move_file(&path, &target)
                .await
                .map(|_| ())
                .map_err(anyhow::Error::from)
        };
        if let Err(error) = result {
            tracing::warn!(
                path = %path.display(),
                %error,
                "leftover was not carried over to the new category directory"
            );
        }
    }
}

/// Removes what is left of a package's former directory once its data has moved on.
///
/// Both removals only take an empty directory, so anything the user kept there survives. Two
/// directories are deliberately spared: the service download directory, and the parent of the
/// new destination — that is the shape a row written before packages had their own folder has,
/// and it is the category directory itself, shared with every other package in it.
async fn sweep_former_directory(
    from: &Path,
    to: &Path,
    downloads_directory: &Path,
    import_id: Option<NzbImportId>,
) {
    if from == downloads_directory || to.parent() == Some(from) {
        return;
    }
    crate::finish::remove_if_empty(&staging_directory(from, import_id)).await;
    if import_id.is_some() {
        crate::finish::remove_if_empty(&staging_directory(from, None)).await;
    }
    crate::finish::remove_if_empty(from).await;
}
