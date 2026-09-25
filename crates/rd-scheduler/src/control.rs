use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rd_core::{DownloadFile, DownloadId, DownloadState, NzbImportId, PackageId, StorageRootId};
use rd_db::StoreError;
use rd_files::{StorageRoot, collision_free_path, move_directory, move_file};

use crate::{SchedulerHandle, StopReason};

impl SchedulerHandle {
    /// Requests a safe pause at the next checkpoint boundary.
    pub async fn pause(&self, id: DownloadId) -> Result<()> {
        let token = {
            let mut active = self.active.lock().await;
            active.reasons.insert(id, StopReason::Paused);
            active.tokens.get(&id).cloned()
        };
        if let Some(token) = token {
            token.cancel();
        } else {
            let current = self
                .database
                .get_download(id)
                .await?
                .context(StoreError::not_found("download not found"))?;
            if matches!(
                current.state,
                DownloadState::Queued | DownloadState::RetryWait
            ) {
                self.database
                    .transition_download(id, DownloadState::Paused)
                    .await?;
            }
        }
        Ok(())
    }

    /// Moves a paused, failed, blocked or cancelled job back to the queue.
    pub async fn resume(&self, id: DownloadId) -> Result<()> {
        self.active.lock().await.reasons.remove(&id);
        // Starting a waiting mirror by hand is a decision about which link to use, so its
        // siblings stand down for it. Without this the dispatcher would put it straight back:
        // it picks the group's member by the same rule that chose the current one.
        self.stand_down_siblings_of(id).await?;
        self.database
            .transition_download(id, DownloadState::Queued)
            .await?;
        Ok(())
    }

    /// Stands the other members of `id`'s mirror group down, so this one gets the turn.
    ///
    /// Only members that have not started: one that is already downloading keeps what it has
    /// done, and the dispatcher settles the pair on its next pass rather than throwing work
    /// away here.
    async fn stand_down_siblings_of(&self, id: DownloadId) -> Result<()> {
        let Some(file) = self.database.get_download(id).await? else {
            return Ok(());
        };
        if file.mirror_group.is_none() || file.state != DownloadState::Skipped {
            return Ok(());
        }
        let downloads = self.database.list_downloads().await?;
        let siblings = crate::mirrors::siblings(&file, &downloads);
        // Refused rather than silently reverted: the dispatcher would put this one straight
        // back and nothing would say why. Throwing away a transfer that is already under way
        // is not something to do on a resume click either.
        if let Some(active) = siblings
            .iter()
            .find(|sibling| crate::mirrors::has_taken_the_turn(sibling.state))
        {
            bail!(
                "another link to this file is already downloading: {}",
                active.source
            );
        }
        for sibling in siblings {
            if crate::mirrors::is_contending(sibling.state) {
                self.database
                    .transition_download(sibling.id, DownloadState::Skipped)
                    .await?;
            }
        }
        Ok(())
    }

    /// Cancels a queued or active job without deleting partial data.
    pub async fn cancel(&self, id: DownloadId) -> Result<()> {
        let token = {
            let mut active = self.active.lock().await;
            active.reasons.insert(id, StopReason::Cancelled);
            active.tokens.get(&id).cloned()
        };
        if let Some(token) = token {
            token.cancel();
        } else {
            self.database
                .transition_download(id, DownloadState::Cancelled)
                .await?;
        }
        // Cancelling the member that held the group's turn is as final as running out of
        // retries; without this its mirrors wait for a link that is never coming back.
        self.promote_mirror_of(id).await;
        Ok(())
    }

    /// Lets a waiting mirror take over when the member holding the turn steps aside.
    ///
    /// Failure to promote is logged rather than propagated: it must not turn a cancel or a
    /// removal that already happened into an error the caller has to undo.
    async fn promote_mirror_of(&self, id: DownloadId) {
        let Ok(Some(file)) = self.database.get_download(id).await else {
            return;
        };
        if file.mirror_group.is_none() {
            return;
        }
        if let Err(error) = crate::failures::wake_mirror(self, &file).await {
            tracing::warn!(%error, download_id = %id, "no mirror could take over");
        }
    }

    /// Removes an inactive queue entry and its incomplete staging file.
    pub async fn remove(&self, id: DownloadId) -> Result<()> {
        let current = self
            .database
            .get_download(id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if is_active(current.state) {
            bail!(StoreError::wrong_state(
                "active download must be paused or cancelled before removal"
            ));
        }
        {
            let mut active = self.active.lock().await;
            if active.tokens.contains_key(&id) {
                bail!(StoreError::wrong_state(
                    "active download must be paused or cancelled before removal"
                ));
            }
            active.reasons.insert(id, StopReason::Cancelled);
        }
        let package = self
            .database
            .list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == current.package_id)
            .filter(|package| !package.destination.is_empty());
        let usenet_import = package.as_ref().and_then(|package| {
            (current.kind == rd_core::DownloadKind::Usenet)
                .then_some(package.nzb_import_id)
                .flatten()
        });
        if let Some(package) = &package {
            let removal = match (usenet_import, current.nzb_file_id) {
                (Some(import_id), Some(file_id)) => {
                    remove_usenet_part_file(&package.destination, import_id, file_id).await
                }
                _ => remove_part_file(&package.destination, id).await,
            };
            if let Err(error) = removal {
                tracing::warn!(download_id = %id, %error, "incomplete staging file was not removed");
            }
        }
        // Read before the row is gone: after the delete there is nothing left to say which
        // group it belonged to.
        let mirror_of = current.mirror_group.is_some().then(|| current.clone());
        self.database.delete_download(id).await?;
        if let Some(removed) = mirror_of
            && let Err(error) = crate::failures::wake_mirror(self, &removed).await
        {
            tracing::warn!(%error, download_id = %id, "no mirror could take over");
        }
        // The package row disappears with its last file; only then may its directory go, and
        // only while it is empty — data the user kept there stays untouched.
        if let Some(package) = package
            && Path::new(&package.destination) != self.config.downloads_directory
            && !self
                .database
                .list_packages()
                .await?
                .iter()
                .any(|remaining| remaining.id == package.id)
        {
            remove_empty_package_directory(&package.destination, usenet_import).await;
        }
        Ok(())
    }

    /// Discards everything a job produced and puts it back at the start of the queue.
    ///
    /// `delete_completed_files` decides whether a finished payload goes with it. Keeping it
    /// means the fresh attempt lands beside it under a collision-free name instead of
    /// overwriting it, which is what somebody who resets to get a second copy expects.
    ///
    /// Nothing has to be started afterwards: the supervisor picks up a `queued` row by itself.
    pub async fn reset(&self, id: DownloadId, delete_completed_files: bool) -> Result<()> {
        let current = self
            .database
            .get_download(id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if is_active(current.state) {
            bail!(StoreError::wrong_state(
                "active download must be paused or cancelled before it can be reset"
            ));
        }
        {
            let mut active = self.active.lock().await;
            if active.tokens.contains_key(&id) {
                bail!(StoreError::wrong_state(
                    "active download must be paused or cancelled before it can be reset"
                ));
            }
            // The pause or cancel that made this reset legal left a stop reason behind, and
            // the dispatcher skips every id that has one (`schedule_runnable`). Without this
            // the row goes back to `queued` and is then never picked up again, with no error
            // anywhere, until the process restarts.
            active.reasons.remove(&id);
        }
        let package = self
            .database
            .list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == current.package_id)
            .filter(|package| !package.destination.is_empty());
        if let Some(package) = &package {
            let usenet_import = (current.kind == rd_core::DownloadKind::Usenet)
                .then_some(package.nzb_import_id)
                .flatten();
            let removal = match (usenet_import, current.nzb_file_id) {
                (Some(import_id), Some(file_id)) => {
                    remove_usenet_part_file(&package.destination, import_id, file_id).await
                }
                _ => remove_part_file(&package.destination, id).await,
            };
            if let Err(error) = removal {
                tracing::warn!(download_id = %id, %error, "incomplete staging file was not discarded");
            }
            if let Err(error) = discard_scratch_files(&package.destination, &current).await {
                tracing::warn!(download_id = %id, %error, "leftover scratch files were not discarded");
            }
            if delete_completed_files {
                let payload = Path::new(&package.destination).join(&current.file_name);
                if let Err(error) = remove_file_if_present(&payload).await {
                    tracing::warn!(download_id = %id, %error, "finished file was not discarded");
                }
            }
        }
        self.database.reset_download(id).await?;
        Ok(())
    }

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
        let Some(previous) = self
            .database
            .package_previous_destination(package_id)
            .await?
        else {
            return Ok(());
        };
        let Some(package) = self
            .database
            .list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == package_id)
        else {
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
        let files: Vec<DownloadFile> = self
            .database
            .list_downloads()
            .await?
            .into_iter()
            .filter(|file| file.package_id == package_id)
            .collect();
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
            // A file of the same name may already sit in the new category folder; it belongs to
            // somebody else and is never overwritten by a move.
            let target = collision_free_path(to, &file.file_name);
            move_file(&payload, &target).await?;
            if target.file_name() != payload.file_name()
                && let Some(name) = target.file_name().and_then(|value| value.to_str())
            {
                self.database
                    .set_download_file_name(file.id, name.to_owned())
                    .await?;
            }
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
            move_file(&path, &target).await
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

fn is_active(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Repairing
            | DownloadState::Extracting
    )
}

/// Opens the package destination for cleanup, or `None` when it no longer exists.
///
/// Deliberately not `StorageRoot::create`: removing a download must never recreate a
/// destination the user has already moved away.
async fn open_destination(destination: &str) -> Result<Option<StorageRoot>> {
    StorageRoot::open_existing(
        StorageRootId::new(),
        "download destination".to_owned(),
        Path::new(destination).to_owned(),
    )
    .await
}

/// The staging directory a file of this kind writes its `.part` into.
fn staging_of(root: &StorageRoot, import_id: Option<rd_core::NzbImportId>) -> Result<PathBuf> {
    match import_id {
        Some(import_id) => root.resolve(Path::new(&format!(".rdownloader-{import_id}"))),
        None => root.resolve(Path::new(".rdownloader")),
    }
}

pub(crate) async fn remove_part_file(destination: &str, id: DownloadId) -> Result<()> {
    let Some(root) = open_destination(destination).await? else {
        return Ok(());
    };
    let staging = staging_of(&root, None)?;
    let part = staging.join(format!("{id}.part"));
    remove_file_if_present(&part).await
}

async fn remove_usenet_part_file(
    destination: &str,
    import_id: rd_core::NzbImportId,
    file_id: rd_core::NzbFileId,
) -> Result<()> {
    let Some(root) = open_destination(destination).await? else {
        return Ok(());
    };
    let staging = staging_of(&root, Some(import_id))?;
    remove_file_if_present(&staging.join(format!("{file_id}.part"))).await
}

async fn remove_file_if_present(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Removes the scratch files an external tool leaves beside the target file.
///
/// yt-dlp writes its stream fragments as `<stem>.f137.mp4.part` and its resume state as
/// `<stem>.info.json.ytdl` next to the output rather than into the staging directory, so a
/// reset that ignored them would resume a half-merged stream. Only those two suffixes are
/// considered, and only below the file's own stem — everything else in a package folder is
/// downloaded data.
async fn discard_scratch_files(destination: &str, file: &rd_core::DownloadFile) -> Result<()> {
    let stem = Path::new(&file.file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file.file_name.as_str());
    if stem.is_empty() {
        return Ok(());
    }
    let Some(root) = open_destination(destination).await? else {
        return Ok(());
    };
    let mut entries = tokio::fs::read_dir(root.path()).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(stem) && (name.ends_with(".part") || name.ends_with(".ytdl")) {
            remove_file_if_present(&entry.path()).await?;
        }
    }
    Ok(())
}

/// Drops the staging directory and the package directory once the package's last file is
/// gone — but only while they are empty, so downloaded data is never touched.
async fn remove_empty_package_directory(
    destination: &str,
    import_id: Option<rd_core::NzbImportId>,
) {
    let root = match open_destination(destination).await {
        Ok(Some(root)) => root,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(path = %destination, %error, "package directory was not inspected");
            return;
        }
    };
    if let Ok(staging) = staging_of(&root, import_id) {
        crate::finish::remove_if_empty(&staging).await;
    }
    crate::finish::remove_if_empty(root.path()).await;
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use rd_core::DownloadFile;

    use crate::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

    /// A scheduler over a temporary database, plus one paused package below `storage/`.
    async fn paused_package(directory: &Path) -> (SchedulerHandle, DownloadFile, PathBuf) {
        let database = rd_db::Database::open(directory.join("scheduler-test.sqlite3"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
            .await
            .expect("secrets");
        let scheduler = SchedulerHandle::start(
            database,
            SchedulerConfig::for_directory(directory.join("downloads")),
            secrets,
            None,
            Vec::new(),
        )
        .await
        .expect("scheduler");
        let (_package, files) = scheduler
            .enqueue_package(
                PackageSpec {
                    name: "Example Package".to_owned(),
                    destination: directory.join("storage"),
                    category_id: None,
                    priority: rd_core::DownloadPriority::default(),
                    password: None,
                    // Paused, so the supervisor leaves the file alone while the test works on it.
                    start_paused: true,
                    postprocess_level: None,
                    script: None,
                    enrichment: Vec::new(),
                },
                vec![FileSpec {
                    source: "https://example.invalid/file.bin".parse().expect("url"),
                    file_name: "file.bin".to_owned(),
                    size: None,
                    account_id: None,
                    proxy_profile_id: None,
                    auth_profile: rd_core::AuthProfileSelection::Auto,
                    kind: rd_core::DownloadKind::Http,
                    media: None,
                    remote_credential_id: None,
                    replay: None,
                    mirror_group: None,
                    skipped: false,
                    enrichment: Vec::new(),
                    secret_fragment: None,
                }],
            )
            .await
            .expect("enqueue");
        let file = files.into_iter().next().expect("one file");
        let destination = directory.join("storage").join("Example Package");
        (scheduler, file, destination)
    }

    /// Points the package at `destination` the way a category change does, recording where its
    /// data used to be.
    async fn set_destination(
        scheduler: &SchedulerHandle,
        package_id: rd_core::PackageId,
        destination: &Path,
    ) {
        scheduler
            .database
            .update_packages(
                vec![package_id],
                rd_db::PackageChange {
                    category: Some(rd_db::CategoryAssignment {
                        category_id: None,
                        destinations: std::collections::HashMap::from([(
                            package_id,
                            destination.to_string_lossy().into_owned(),
                        )]),
                    }),
                    ..Default::default()
                },
            )
            .await
            .expect("category change");
    }

    #[tokio::test]
    async fn a_category_change_carries_the_package_folder_over() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        tokio::fs::create_dir_all(&destination)
            .await
            .expect("destination");
        tokio::fs::write(destination.join(&file.file_name), b"payload")
            .await
            .expect("payload");

        let moved = temporary.path().join("movies").join("Example Package");
        set_destination(&scheduler, file.package_id, &moved).await;
        scheduler
            .relocate_package(file.package_id)
            .await
            .expect("relocate");

        assert_eq!(
            tokio::fs::read(moved.join(&file.file_name))
                .await
                .expect("moved payload"),
            b"payload",
            "the file follows its package into the new category folder"
        );
        assert!(
            !destination.exists(),
            "the emptied package folder does not stay behind"
        );
        assert_eq!(
            scheduler
                .database
                .package_previous_destination(file.package_id)
                .await
                .expect("previous destination"),
            None,
            "and the outstanding move is marked as done"
        );
    }

    #[tokio::test]
    async fn a_job_holding_both_a_payload_and_a_checkpoint_carries_both_over() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        let staging = destination.join(".rdownloader");
        tokio::fs::create_dir_all(&staging).await.expect("staging");
        tokio::fs::write(destination.join(&file.file_name), b"payload")
            .await
            .expect("payload");
        let part = staging.join(format!("{}.part", file.id));
        tokio::fs::write(&part, b"partial")
            .await
            .expect("part file");

        let moved = temporary.path().join("movies").join("Example Package");
        set_destination(&scheduler, file.package_id, &moved).await;
        scheduler
            .relocate_package(file.package_id)
            .await
            .expect("relocate");

        assert!(
            moved.join(&file.file_name).exists(),
            "the payload moves with the package"
        );
        assert!(
            moved
                .join(".rdownloader")
                .join(format!("{}.part", file.id))
                .exists(),
            "and so does the checkpoint, which is not an alternative to the payload"
        );
        assert!(
            !destination.exists(),
            "so nothing is left to keep the old directory alive"
        );
    }

    /// A folder that belongs to one package moves with it, contents and all.
    ///
    /// Not only the rows the database knows about: extracted output has no download row, and
    /// archive parts can sit in a subfolder that was never descended into, so both used to
    /// stay behind while the package record claimed to have moved. Anything else in a folder
    /// that belongs to this package alone is treated the same way, because there is no way to
    /// tell it apart from the extraction output it sits next to.
    #[tokio::test]
    async fn everything_in_a_package_folder_moves_with_the_package() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        tokio::fs::create_dir_all(&destination)
            .await
            .expect("destination");
        tokio::fs::write(destination.join(&file.file_name), b"payload")
            .await
            .expect("payload");
        // Stands in for extracted output: real content, no row in the database.
        let unknown = destination.join("notes.txt");
        tokio::fs::write(&unknown, b"mine")
            .await
            .expect("extra file");

        let moved = temporary.path().join("movies").join("Example Package");
        set_destination(&scheduler, file.package_id, &moved).await;
        scheduler
            .relocate_package(file.package_id)
            .await
            .expect("relocate");

        assert!(
            moved.join(&file.file_name).exists(),
            "the payload moves with the package"
        );
        assert!(
            moved.join("notes.txt").exists(),
            "and so does what the database never knew about"
        );
        assert!(
            !destination.exists(),
            "leaving nothing behind to keep the old directory alive"
        );
    }

    #[tokio::test]
    async fn a_row_that_still_points_at_the_category_root_keeps_the_category_folder() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, _) = paused_package(temporary.path()).await;
        // The shape of a package written before the category change built a folder per package:
        // its destination *is* the category directory, shared with everything else in it.
        let category = temporary.path().join("storage");
        set_destination(&scheduler, file.package_id, &category).await;
        scheduler
            .database
            .clear_package_previous_destination(file.package_id)
            .await
            .expect("clear");
        tokio::fs::create_dir_all(&category)
            .await
            .expect("category");
        tokio::fs::write(category.join(&file.file_name), b"payload")
            .await
            .expect("payload");
        let sibling = category.join("another-package.bin");
        tokio::fs::write(&sibling, b"other").await.expect("sibling");

        let moved = category.join("Example Package");
        set_destination(&scheduler, file.package_id, &moved).await;
        scheduler
            .relocate_package(file.package_id)
            .await
            .expect("relocate");

        assert_eq!(
            tokio::fs::read(moved.join(&file.file_name))
                .await
                .expect("moved payload"),
            b"payload",
            "only the package's own file is carried into its new folder"
        );
        assert!(
            sibling.exists(),
            "a file belonging to another package stays where it is"
        );
        assert!(
            category.is_dir(),
            "and the category directory itself is never swept away"
        );
    }

    #[tokio::test]
    async fn resetting_discards_the_partial_file_and_queues_the_job_again() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        let staging = destination.join(".rdownloader");
        tokio::fs::create_dir_all(&staging).await.expect("staging");
        let part = staging.join(format!("{}.part", file.id));
        tokio::fs::write(&part, b"partial")
            .await
            .expect("part file");

        scheduler.reset(file.id, false).await.expect("reset");

        assert!(!part.exists(), "the checkpoint file is discarded");
        let current = scheduler
            .database
            .get_download(file.id)
            .await
            .expect("download")
            .expect("still there");
        assert_eq!(current.state, rd_core::DownloadState::Queued);
        assert_eq!(current.committed_bytes.get(), 0, "progress starts at zero");
        assert_eq!(current.retry_count, 0, "and so does the retry budget");
    }

    #[tokio::test]
    async fn a_reset_keeps_the_finished_file_unless_it_is_asked_to_delete_it() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        tokio::fs::create_dir_all(&destination)
            .await
            .expect("destination");
        let payload = destination.join(&file.file_name);
        tokio::fs::write(&payload, b"payload")
            .await
            .expect("payload");

        scheduler.reset(file.id, false).await.expect("reset");
        assert!(
            payload.exists(),
            "a reset does not destroy the only copy by default"
        );

        scheduler.reset(file.id, true).await.expect("reset");
        assert!(
            !payload.exists(),
            "and removes it when that is what was asked for"
        );
    }

    #[tokio::test]
    async fn removing_a_download_does_not_recreate_a_destination_that_was_moved_away() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        assert!(!destination.exists(), "nothing was downloaded yet");

        scheduler.remove(file.id).await.expect("remove");

        assert!(
            !destination.exists(),
            "removing a download must not create its package directory"
        );
    }

    #[tokio::test]
    async fn removing_the_last_download_drops_the_empty_package_directory() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        let staging = destination.join(".rdownloader");
        tokio::fs::create_dir_all(&staging).await.expect("staging");
        tokio::fs::write(staging.join(format!("{}.part", file.id)), b"partial")
            .await
            .expect("part file");

        scheduler.remove(file.id).await.expect("remove");

        assert!(
            !destination.exists(),
            "an empty package directory is cleaned up with its last file"
        );
    }

    #[tokio::test]
    async fn a_package_directory_that_still_holds_data_survives() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let (scheduler, file, destination) = paused_package(temporary.path()).await;
        tokio::fs::create_dir_all(&destination)
            .await
            .expect("destination");
        let kept = destination.join("already-downloaded.bin");
        tokio::fs::write(&kept, b"payload").await.expect("payload");

        scheduler.remove(file.id).await.expect("remove");

        assert!(kept.exists(), "downloaded data is never removed");
    }
}
