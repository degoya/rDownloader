//! Dissolving a single folder named like the package (RD-1140-01).
//!
//! A release whose archive carries its own folder lands as `Release/Release/…` once it is
//! unpacked, and so does an NZB or a torrent that brings that folder along. With the setting on,
//! a package folder holding nothing but one folder named like itself takes that folder's content
//! one level up, and the emptied folder goes; with a folder per archive (RD-170-16) the same holds
//! for each archive's folder, `X/` with nothing but `X/` in it. One level, never more. It runs
//! after the unpack, the archives' deletion and the cleanup, and before everything that hands the
//! package on — the scan, the plugin steps, the user script, the upload and the sort.
//!
//! Nothing is overwritten and no link is followed. The folder is first renamed to [`STAGING`], so
//! its parent holds nothing else while the entries move up one at a time, each only to a name that
//! is free; a stop between two moves leaves that folder behind, and the next pass finishes the
//! moves before it looks for anything new. Names are compared after the sanitising a package
//! folder's name gets (`rd_files::package_directory`), without regard to case.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{DownloadKind, DownloadState, PostprocessKind, PostprocessState};

use crate::{
    package_job::Run,
    steps::{Outcome, checkpoint_coded, codes, truncate},
    unpack_job::UnpackTarget,
};

/// What the folder being dissolved is called while its content moves up. Not a `.rd-x` staging
/// name: those are swept as leftovers, and this one holds the package's files.
pub(crate) const STAGING: &str = ".rd-unwrap";

/// The step a problem is reported on, beside the cleanup it continues.
pub(crate) const SOURCE: &str = "unwrap";

/// Why a folder stayed, wholly or in part, where it was.
enum Problem {
    /// This name already exists where an entry would have moved.
    Conflict(String),
    /// The file system refused; its own words.
    Failed(String),
}

/// Finishes a dissolve a stop interrupted, whatever the setting says now, then — when `enabled`
/// — dissolves what qualifies.
pub(crate) async fn run(run: &Run<'_>, enabled: bool) -> Result<()> {
    let folders = archive_folders(run);
    for parent in std::iter::once(&run.directory).chain(&folders) {
        let staging = parent.join(STAGING);
        if is_folder(&staging).await {
            let problem = lift(parent, &staging).await?;
            report(run, STAGING, problem).await?;
        }
    }
    if !enabled {
        return Ok(());
    }
    for folder in &folders {
        dissolve(run, folder, &[]).await?;
    }
    // A seeding torrent's payload stays where its client reads it; the folders its archives were
    // unpacked into hold none of it.
    let seeding = run.package.kind == DownloadKind::Torrent
        && run
            .downloads
            .iter()
            .any(|file| file.state == DownloadState::Seeding);
    if !seeding {
        // A folder an archive was unpacked into is what a folder per archive asked for, even when
        // the archive is named like the package.
        dissolve(run, &run.directory, &folders).await?;
    }
    Ok(())
}

/// The folders the package's archive sets were unpacked into, when it has one per archive.
fn archive_folders(run: &Run<'_>) -> Vec<PathBuf> {
    if !run.chosen.unpack_to_subfolder || !run.chosen.level.unpacks() || run.sets.is_empty() {
        return Vec::new();
    }
    // A folder that exists is handed out again, so these are the ones the unpack used.
    UnpackTarget::OwnFolder.destinations(&run.directory, &run.sets)
}

/// Dissolves the one folder in `parent` that is named like `parent`, unless it is one of `keep`.
async fn dissolve(run: &Run<'_>, parent: &Path, keep: &[PathBuf]) -> Result<()> {
    let Some(name) = single_folder(parent).await else {
        return Ok(());
    };
    let folder = parent.join(&name);
    if keep.contains(&folder) || !named_like(parent, &name) {
        return Ok(());
    }
    let label = name.to_string_lossy().into_owned();
    // Checked before anything moves, so a conflict leaves everything as it was.
    match first_taken(parent, &folder, &name).await {
        Ok(None) => {}
        Ok(Some(taken)) => return report(run, &label, Some(Problem::Conflict(taken))).await,
        Err(error) => return report(run, &label, Some(Problem::Failed(error.to_string()))).await,
    }
    let staging = parent.join(STAGING);
    if let Err(error) = tokio::fs::rename(&folder, &staging).await {
        return report(run, &label, Some(Problem::Failed(error.to_string()))).await;
    }
    let problem = lift(parent, &staging).await?;
    report(run, &label, problem).await
}

/// The name of the only entry in `parent`, when that entry is a folder and not a link.
async fn single_folder(parent: &Path) -> Option<OsString> {
    let mut entries = tokio::fs::read_dir(parent).await.ok()?;
    let first = entries.next_entry().await.ok()??;
    if entries.next_entry().await.ok()?.is_some() {
        return None;
    }
    // `DirEntry::file_type` does not follow a link, so a link to a folder is not one.
    first
        .file_type()
        .await
        .ok()?
        .is_dir()
        .then(|| first.file_name())
}

/// Whether `name` becomes `parent`'s own name under the sanitising a package folder's name gets.
fn named_like(parent: &Path, name: &OsStr) -> bool {
    let (Some(own), Some(base), Some(name)) = (parent.file_name(), parent.parent(), name.to_str())
    else {
        return false;
    };
    rd_files::package_directory(base, name)
        .file_name()
        .is_some_and(|folder| same_name(folder, own))
}

fn same_name(left: &OsStr, right: &OsStr) -> bool {
    left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
}

/// The first entry of `folder` whose name is the staging name, or is taken in `parent` by
/// anything but `folder` itself.
async fn first_taken(parent: &Path, folder: &Path, own: &OsStr) -> std::io::Result<Option<String>> {
    let mut entries = tokio::fs::read_dir(folder).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        let taken = same_name(&name, OsStr::new(STAGING))
            || (!same_name(&name, own)
                && tokio::fs::symlink_metadata(parent.join(&name))
                    .await
                    .is_ok());
        if taken {
            return Ok(Some(name.to_string_lossy().into_owned()));
        }
    }
    Ok(None)
}

/// Moves every entry of `staging` up into `parent`, each only to a free name, and removes
/// `staging` once it is empty. An entry whose name is taken stays, and is the conflict reported.
async fn lift(parent: &Path, staging: &Path) -> Result<Option<Problem>> {
    let names = match entry_names(staging).await {
        Ok(names) => names,
        Err(error) => return Ok(Some(Problem::Failed(error.to_string()))),
    };
    let mut conflict = None;
    for name in names {
        let target = parent.join(&name);
        if tokio::fs::symlink_metadata(&target).await.is_ok() {
            if conflict.is_none() {
                conflict = Some(name.to_string_lossy().into_owned());
            }
            continue;
        }
        if let Err(error) = move_entry(&staging.join(&name), &target).await {
            return Ok(Some(Problem::Failed(format!("{error:#}"))));
        }
        // A stop between two moves leaves the staging folder; the next pass moves the rest.
        rd_core::failpoint!("postprocess.after_unwrap_move", || anyhow::anyhow!(
            "crash point postprocess.after_unwrap_move"
        ));
    }
    if let Some(name) = conflict {
        return Ok(Some(Problem::Conflict(name)));
    }
    Ok(tokio::fs::remove_dir(staging)
        .await
        .err()
        .map(|error| Problem::Failed(error.to_string())))
}

/// The names in `folder`, read before anything moves out of it.
async fn entry_names(folder: &Path) -> std::io::Result<Vec<OsString>> {
    let mut entries = tokio::fs::read_dir(folder).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        names.push(entry.file_name());
    }
    names.sort();
    Ok(names)
}

/// One entry one level up: a folder with its subtree, a file, or a link as the link itself.
async fn move_entry(from: &Path, to: &Path) -> Result<()> {
    let kind = tokio::fs::symlink_metadata(from).await?.file_type();
    if kind.is_symlink() {
        // Renamed, never copied: a copy across devices would follow it.
        tokio::fs::rename(from, to).await?;
    } else if kind.is_dir() {
        rd_files::move_directory(from, to).await?;
    } else {
        rd_files::move_file(from, to).await?;
    }
    Ok(())
}

async fn is_folder(path: &Path) -> bool {
    tokio::fs::symlink_metadata(path)
        .await
        .is_ok_and(|meta| meta.is_dir())
}

/// Records a problem on the package's step list, where its other steps say what they did.
async fn report(run: &Run<'_>, folder: &str, problem: Option<Problem>) -> Result<()> {
    let Some(problem) = problem else {
        return Ok(());
    };
    let outcome = match problem {
        Problem::Conflict(name) => {
            tracing::warn!(package_id = %run.package_id, %folder, %name, "a folder named like the package stays: a name inside it is taken one level up");
            Outcome::new(
                codes::UNWRAP_CONFLICT,
                &[("folder", folder.to_owned()), ("name", name.clone())],
                format!("{folder} stays as it is: {name} already exists one level up"),
            )
        }
        Problem::Failed(detail) => {
            tracing::warn!(package_id = %run.package_id, %folder, %detail, "dissolving a folder named like the package failed");
            let detail = truncate(detail);
            Outcome::new(
                codes::UNWRAP_FAILED,
                &[("folder", folder.to_owned()), ("detail", detail.clone())],
                format!("{folder} could not be dissolved: {detail}"),
            )
        }
    };
    checkpoint_coded(
        run.inner,
        &run.owner,
        PostprocessKind::Cleanup,
        SOURCE,
        PostprocessState::Skipped,
        None,
        outcome,
    )
    .await
}
