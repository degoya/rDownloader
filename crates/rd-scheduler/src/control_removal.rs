//! Removing a job, resetting it and discarding what it wrote, with the staging and scratch
//! files those leave behind.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rd_core::{DownloadFile, DownloadId, DownloadState, StorageRootId};
use rd_db::StoreError;
use rd_files::StorageRoot;

use super::NzbDropped;
use crate::SchedulerHandle;

/// Why a removal is refused while a worker holds the row.
const REMOVAL_REFUSAL: &str = "active download must be paused or cancelled before removal";

/// What a removal needs once its row is gone, read while the row was still there.
struct StagedRemoval {
    current: DownloadFile,
    /// The package the row belonged to, when it has a folder.
    package: Option<rd_core::DownloadPackage>,
    usenet_import: Option<rd_core::NzbImportId>,
}

impl SchedulerHandle {
    /// Removes an inactive queue entry and its incomplete staging file.
    pub async fn remove(&self, id: DownloadId) -> Result<()> {
        let current = self.removable(id).await?;
        self.while_held(id, REMOVAL_REFUSAL, self.remove_idle(current))
            .await
    }

    /// [`Self::remove`] for many rows (RD-1120-17): the checks, the holds and the staging
    /// files row by row as there, the rows themselves in one writer transaction instead of one
    /// each. One answer per id, in order; a refused row leaves the others to go ahead, and a
    /// second mention of an id finds its row gone.
    pub async fn remove_many(&self, ids: &[DownloadId]) -> Vec<Result<()>> {
        let mut outcomes = Vec::with_capacity(ids.len());
        let mut staged: Vec<(usize, StagedRemoval)> = Vec::new();
        for &id in ids {
            match self.stage_removal(id, &staged).await {
                Ok(entry) => {
                    staged.push((outcomes.len(), entry));
                    outcomes.push(Ok(()));
                }
                Err(error) => outcomes.push(Err(error)),
            }
        }
        if staged.is_empty() {
            return outcomes;
        }
        let rows = staged.iter().map(|(_, entry)| entry.current.id).collect();
        let answers = match self.database.delete_downloads(rows).await {
            Ok(answers) => answers,
            // The transaction failed as a whole: no row of the batch is gone.
            Err(error) => {
                let message = format!("{error:#}");
                staged
                    .iter()
                    .map(|_| Err(anyhow::anyhow!(message.clone())))
                    .collect()
            }
        };
        let mut directories_done = Vec::new();
        for ((index, entry), answer) in staged.into_iter().zip(answers) {
            let id = entry.current.id;
            let answer = match answer {
                Ok(()) => self.after_removal(entry, &mut directories_done).await,
                Err(error) => Err(error),
            };
            if let Some(slot) = outcomes.get_mut(index) {
                *slot = answer;
            }
            self.let_go(id).await;
        }
        outcomes
    }

    /// One row of [`Self::remove_many`] up to its delete: checked, held and its staging file
    /// gone. The hold is let go again when this refuses.
    async fn stage_removal(
        &self,
        id: DownloadId,
        staged: &[(usize, StagedRemoval)],
    ) -> Result<StagedRemoval> {
        if staged.iter().any(|(_, entry)| entry.current.id == id) {
            bail!(StoreError::not_found("download not found"));
        }
        let current = self.removable(id).await?;
        self.hold(id, REMOVAL_REFUSAL).await?;
        match self.clear_staging(current).await {
            Ok(entry) => Ok(entry),
            Err(error) => {
                self.let_go(id).await;
                Err(error)
            }
        }
    }

    /// The row `id` names, once nothing works on it any more; refused while something does.
    async fn removable(&self, id: DownloadId) -> Result<DownloadFile> {
        let mut current = self
            .database
            .get_download(id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        // A Usenet file waiting for its set's PAR2 verdict (RD-108-24) is `Verifying` with no
        // worker behind it: nothing is running that could be stopped first, and it cannot be
        // paused, so asking for a cancel before the removal would leave it undeletable.
        if current.state == DownloadState::Verifying
            && !self.active.lock().await.tokens.contains_key(&id)
        {
            if let Err(error) = self
                .database
                .transition_download(id, DownloadState::Cancelled)
                .await
            {
                // The verdict may have been taken in between; the row is read again below.
                tracing::debug!(download_id = %id, %error, "waiting row was not cancelled");
            }
            current = self
                .database
                .get_download(id)
                .await?
                .context(StoreError::not_found("download not found"))?;
        }
        if current.state.is_working() {
            bail!(StoreError::wrong_state(REMOVAL_REFUSAL));
        }
        Ok(current)
    }

    /// The part of [`Self::remove`] that runs while the stop reason keeps the dispatcher off.
    async fn remove_idle(&self, current: DownloadFile) -> Result<()> {
        let staged = self.clear_staging(current).await?;
        self.database.delete_download(staged.current.id).await?;
        self.after_removal(staged, &mut Vec::new()).await
    }

    /// Deletes the row's incomplete staging file and keeps what the removal needs afterwards.
    async fn clear_staging(&self, current: DownloadFile) -> Result<StagedRemoval> {
        let id = current.id;
        let package = self
            .database
            .get_package(current.package_id)
            .await?
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
        Ok(StagedRemoval {
            current,
            package,
            usenet_import,
        })
    }

    /// What follows a deleted row: a waiting mirror takes the turn, and the package folder
    /// goes once the package has. `directories_done` names the packages whose folder a batch
    /// already looked at, so many files of one package look once.
    async fn after_removal(
        &self,
        staged: StagedRemoval,
        directories_done: &mut Vec<rd_core::PackageId>,
    ) -> Result<()> {
        let StagedRemoval {
            current,
            package,
            usenet_import,
        } = staged;
        if current.mirror_group.is_some()
            && let Err(error) = crate::failures::wake_mirror(self, &current).await
        {
            tracing::warn!(%error, download_id = %current.id, "no mirror could take over");
        }
        // The package row disappears with its last file; only then may its directory go, and
        // only while it is empty — data the user kept there stays untouched.
        if let Some(package) = package
            && !directories_done.contains(&package.id)
            && Path::new(&package.destination) != self.config.downloads_directory
            && self.database.get_package(package.id).await?.is_none()
        {
            directories_done.push(package.id);
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
        if current.state.is_working() {
            bail!(StoreError::wrong_state(
                "active download must be paused or cancelled before it can be reset"
            ));
        }
        // Asked before anything is deleted: the store refuses the same row in
        // `reset_download`, but only after `discard_unfinished` and `delete_completed_files`
        // had taken the file a refused reset leaves behind (re-audit 1.9.1, RA-DB-01).
        if current.kind == rd_core::DownloadKind::Usenet && current.nzb_file_id.is_none() {
            bail!(NzbDropped);
        }
        // Held until the row is written back, and then let go: the pause or cancel that made
        // this reset legal left a stop reason too, and the dispatcher skips every id that has
        // one, so a reason that outlived the reset kept the `queued` row from ever starting.
        self.while_held(
            id,
            "active download must be paused or cancelled before it can be reset",
            async {
                let packages = self.database.list_packages().await?;
                if let Some(package) = self.discard_unfinished(&current, &packages).await?
                    && delete_completed_files
                {
                    let payload = Path::new(&package.destination).join(&current.file_name);
                    if let Err(error) = remove_file_if_present(&payload).await {
                        tracing::warn!(download_id = %id, %error, "finished file was not discarded");
                    }
                }
                self.database.reset_download(id).await?;
                anyhow::Ok(())
            },
        )
        .await
    }

    /// Deletes what an unfinished, stopped job has written so far, ahead of its removal.
    ///
    /// [`Self::remove`] already takes the staging file; this adds the scratch files an external
    /// tool leaves beside the target, by the rule a reset uses, for the removal that was asked
    /// to throw the work away (RD-180-21). The row stays: data first, row last, so a crash in
    /// between leaves a row that knows less data, never data that no row knows. A finished
    /// payload is not touched, and a job that is still running is refused like a removal is.
    pub async fn discard_partial(&self, id: DownloadId) -> Result<()> {
        let current = self
            .database
            .get_download(id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        const REFUSAL: &str =
            "active download must be paused or cancelled before its data is discarded";
        if current.state.is_working() {
            bail!(StoreError::wrong_state(REFUSAL));
        }
        self.while_held(id, REFUSAL, async {
            let packages = self.database.list_packages().await?;
            self.discard_unfinished(&current, &packages).await?;
            anyhow::Ok(())
        })
        .await
    }

    /// The staging file and the tool scratch files of one job, shared by a reset and
    /// [`Self::discard_partial`]. Answers the package the job writes into, if it has a folder.
    async fn discard_unfinished<'a>(
        &self,
        current: &DownloadFile,
        packages: &'a [rd_core::DownloadPackage],
    ) -> Result<Option<&'a rd_core::DownloadPackage>> {
        let id = current.id;
        let Some(package) = packages
            .iter()
            .find(|package| package.id == current.package_id)
            .filter(|package| !package.destination.is_empty())
        else {
            return Ok(None);
        };
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
        let folder = packages
            .iter()
            .filter(|other| other.destination == package.destination)
            .map(|other| other.id)
            .collect::<std::collections::HashSet<_>>();
        let mut neighbours = Vec::new();
        for package_id in folder {
            neighbours.extend(
                self.database
                    .downloads_for_package(package_id)
                    .await?
                    .into_iter()
                    .filter(|other| other.id != id),
            );
        }
        if let Err(error) = discard_scratch_files(&package.destination, current, &neighbours).await
        {
            tracing::warn!(download_id = %id, %error, "leftover scratch files were not discarded");
        }
        Ok(Some(package))
    }
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
/// considered, and only below the file's own stem and a dot - everything else in a package
/// folder is downloaded data.
///
/// `neighbours` are the other downloads writing into the same folder. A bare prefix match took
/// `Episode 10.f137.mp4.part` for a scratch file of `Episode 1`; the dot rules that out, and a
/// neighbour whose stem is longer (`Episode 1.5`) or the same and still running keeps its files.
pub(super) async fn discard_scratch_files(
    destination: &str,
    file: &rd_core::DownloadFile,
    neighbours: &[rd_core::DownloadFile],
) -> Result<()> {
    let stem = file_stem(&file.file_name);
    if stem.is_empty() {
        return Ok(());
    }
    let own = format!("{stem}.");
    let protected = neighbours
        .iter()
        .filter_map(|neighbour| {
            let other = file_stem(&neighbour.file_name);
            (other.len() > stem.len() || (other == stem && neighbour.state.is_working()))
                .then(|| format!("{other}."))
        })
        .collect::<Vec<_>>();
    let Some(root) = open_destination(destination).await? else {
        return Ok(());
    };
    let mut entries = tokio::fs::read_dir(root.path()).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(&own)
            && (name.ends_with(".part") || name.ends_with(".ytdl"))
            && !protected
                .iter()
                .any(|other| name.starts_with(other.as_str()))
        {
            remove_file_if_present(&entry.path()).await?;
        }
    }
    Ok(())
}

fn file_stem(file_name: &str) -> &str {
    Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(file_name)
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
