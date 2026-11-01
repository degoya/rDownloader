//! A full reset of a failed package (RD-1390-07): everything the package demonstrably put on
//! disk goes, and the package stands in the queue as the LinkGrabber handed it over.
//!
//! Data first, rows last, as every reset and removal here: a stop in between leaves rows that
//! know less data than they say, never data that no row knows - and a package that still reads
//! as failed, so the same reset can be asked again and finishes it.

use std::path::{Component, Path};

use anyhow::{Context, Result, bail};
use rd_core::{DownloadFile, DownloadPackage, DownloadState, PackageId, PackageState};
use rd_db::StoreError;

use super::{
    NzbDropped,
    removal::{remove_empty_package_directory, remove_file_if_present},
};
use crate::SchedulerHandle;

/// Why a package reset is refused while one of its files is at work or seeding.
const REFUSAL: &str = "running downloads must be paused or cancelled before the package is reset";

/// A package reset refused because nothing of the package failed.
///
/// The reset deletes finished files too, so it is offered for a failed package only: one with
/// a failed or blocked file, or whose post-processing failed. A type of its own so the
/// interface can say so instead of "pause it first".
#[derive(Debug)]
pub struct PackageNotFailed;

impl std::fmt::Display for PackageNotFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "only a package with a failed or blocked file, or whose post-processing failed, can be reset whole",
        )
    }
}

impl std::error::Error for PackageNotFailed {}

impl SchedulerHandle {
    /// Resets a failed package whole: every file's staging, scratch and finished data, its
    /// Usenet files' assembled outputs and - with `delete_extracted` - the entries its unpacks
    /// created are deleted, then its empty folder; every file is `queued` again from zero and
    /// the package's own outcome is cleared. What the LinkGrabber chose stays. Answers the
    /// number of files reset.
    ///
    /// Refused before anything is deleted while a file is at work or seeding
    /// ([`StoreError::wrong_state`]), when nothing of the package failed ([`PackageNotFailed`])
    /// and when a Usenet file lost its NZB ([`NzbDropped`]). Only what the package provably
    /// produced is deleted: a file somebody else keeps in a shared folder stays.
    pub async fn reset_package(
        &self,
        package_id: PackageId,
        delete_extracted: bool,
    ) -> Result<usize> {
        let package = self
            .database
            .get_package(package_id)
            .await?
            .context(StoreError::not_found("package not found"))?;
        let files = self.database.downloads_for_package(package_id).await?;
        refuse(&package, &files)?;
        // Every file held, so the dispatcher starts none of them while its data goes, and let
        // go once the rows are written back.
        let mut held = Vec::with_capacity(files.len());
        let mut outcome = Ok(0);
        for file in &files {
            if let Err(error) = self.hold(file.id, REFUSAL).await {
                outcome = Err(error);
                break;
            }
            held.push(file.id);
        }
        if outcome.is_ok() {
            outcome = self.reset_held_package(&package, delete_extracted).await;
        }
        for id in held {
            self.let_go(id).await;
        }
        outcome
    }

    /// The part of [`Self::reset_package`] that runs while every file is held.
    async fn reset_held_package(
        &self,
        package: &DownloadPackage,
        delete_extracted: bool,
    ) -> Result<usize> {
        // Asked again under the holds: a file may have started in between.
        let files = self.database.downloads_for_package(package.id).await?;
        refuse(package, &files)?;
        let plan = self.database.package_reset_plan(package.id).await?;
        let packages = self.database.list_packages().await?;
        let folder = Path::new(&package.destination);
        for file in &files {
            let in_folder = self.discard_unfinished(file, &packages).await?.is_some();
            // Only a finished file is in its final place under its own name: before that the
            // name may still be a stranger's the collision rule moves it away from.
            if in_folder && file.state == DownloadState::Completed {
                remove_payload(folder, file).await?;
            }
        }
        for path in &plan.usenet_outputs {
            remove_entry(folder, Path::new(path)).await?;
        }
        if delete_extracted {
            for path in &plan.unpack_outputs {
                remove_entry(folder, Path::new(path)).await?;
            }
            for unpacked_into in &plan.unpack_folders {
                let unpacked_into = Path::new(unpacked_into);
                if inside(folder, unpacked_into) {
                    crate::finish::remove_if_empty(unpacked_into).await;
                }
            }
        }
        rd_core::failpoint!("scheduler.before_package_reset_rows", || anyhow::anyhow!(
            "crash point: the package's data is gone and its rows are not reset yet"
        ));
        let reset = self
            .database
            .reset_package(package.id, delete_extracted)
            .await?;
        if !package.destination.is_empty() && folder != self.config.downloads_directory {
            remove_empty_package_directory(&package.destination, package.nzb_import_id).await;
        }
        Ok(reset.len())
    }
}

/// The refusals of a package reset, asked before anything is deleted.
fn refuse(package: &DownloadPackage, files: &[DownloadFile]) -> Result<()> {
    if files.iter().any(|file| file.state.holds_the_file()) {
        bail!(StoreError::wrong_state(REFUSAL));
    }
    let failed = package.state == PackageState::Failed
        || files
            .iter()
            .any(|file| matches!(file.state, DownloadState::Failed | DownloadState::Blocked));
    if !failed {
        bail!(PackageNotFailed);
    }
    if files
        .iter()
        .any(|file| file.kind == rd_core::DownloadKind::Usenet && file.nzb_file_id.is_none())
    {
        bail!(NzbDropped);
    }
    Ok(())
}

/// Whether `path` lies below `folder`: absolute, no `..`, and not the folder itself.
fn inside(folder: &Path, path: &Path) -> bool {
    path.is_absolute()
        && path != folder
        && path.starts_with(folder)
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

/// A finished file's payload in the package folder; a torrent's may be a folder of its own.
async fn remove_payload(folder: &Path, file: &DownloadFile) -> Result<()> {
    let payload = folder.join(&file.file_name);
    match tokio::fs::symlink_metadata(&payload).await {
        Ok(metadata) if metadata.is_dir() => {
            if file.kind == rd_core::DownloadKind::Torrent && inside(folder, &payload) {
                tokio::fs::remove_dir_all(&payload).await?;
            }
            Ok(())
        }
        Ok(_) => remove_file_if_present(&payload).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// One recorded entry: a file, or a folder the package created whole. Anything that does not
/// lie below the package folder is left alone, whatever the record says.
async fn remove_entry(folder: &Path, path: &Path) -> Result<()> {
    if !inside(folder, path) {
        tracing::warn!(path = %path.display(), "a recorded entry outside the package folder was left alone");
        return Ok(());
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() => Ok(tokio::fs::remove_dir_all(path).await?),
        Ok(_) => remove_file_if_present(path).await,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
#[path = "control_reset_package_tests.rs"]
mod tests;
