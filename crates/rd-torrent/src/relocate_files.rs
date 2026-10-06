//! The file side of a torrent move: placing, comparing and clearing the torrent's files.

use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};
use rd_core::TorrentJobState;

use super::Relocation;

/// Puts every file of the torrent that is on disk at the new place, originals kept where a
/// copy was needed.
pub(super) async fn place_all(relocation: &Relocation) -> Result<()> {
    for relative in &relocation.files {
        let source = relocation.source_folder().join(relative);
        // Deselected files and padding were never written.
        if !exists(&source).await? {
            continue;
        }
        let target = relocation.target_folder().join(relative);
        create_parent(&target).await?;
        rd_files::place_verified(&source, &target)
            .await
            .with_context(|| format!("move {}", relative.display()))?;
    }
    Ok(())
}

/// The torrent's files relative to its package folder, refused whole if one would leave it.
pub(super) fn torrent_files(state: &TorrentJobState) -> Result<Vec<PathBuf>> {
    let metadata = state
        .metadata
        .as_ref()
        .context("the torrent's file list is not known yet")?;
    metadata
        .files
        .iter()
        .map(|file| {
            relative_path(&file.path).with_context(|| {
                format!("unsafe file path in the torrent: {}", file.display_path())
            })
        })
        .collect()
}

/// Joins path components that are each one plain name; `None` for anything else.
pub(super) fn relative_path(components: &[String]) -> Option<PathBuf> {
    if components.is_empty() {
        return None;
    }
    let mut path = PathBuf::new();
    for component in components {
        let mut parts = Path::new(component).components();
        if !matches!(
            (parts.next(), parts.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return None;
        }
        path.push(component);
    }
    Some(path)
}

/// Removes the folders the torrent's files lived in, deepest first, and `root` itself — each
/// only when it is empty, so nothing that is not the torrent's goes with them.
pub(super) async fn remove_empty_folders(root: &Path, files: &[PathBuf]) {
    let folders: BTreeSet<&Path> = files
        .iter()
        .flat_map(|file| file.ancestors().skip(1))
        .filter(|folder| !folder.as_os_str().is_empty())
        .collect();
    let mut folders: Vec<&Path> = folders.into_iter().collect();
    folders.sort_by_key(|folder| std::cmp::Reverse(folder.components().count()));
    for folder in folders {
        let _ = tokio::fs::remove_dir(root.join(folder)).await;
    }
    let _ = tokio::fs::remove_dir(root).await;
}

/// Whether `path` is no folder yet or an empty one.
pub(super) async fn folder_is_free(path: &Path) -> bool {
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => matches!(entries.next_entry().await, Ok(None)),
        Err(_) => true,
    }
}

/// Whether two files hold the same bytes: the length first, the SHA-256 only when it matches.
pub(super) async fn same_content(first: &Path, second: &Path) -> Result<bool> {
    let (left, right) = (
        tokio::fs::metadata(first).await?,
        tokio::fs::metadata(second).await?,
    );
    if left.len() != right.len() {
        return Ok(false);
    }
    let algorithm = rd_core::ChecksumAlgorithm::Sha256;
    Ok(rd_files::compute_checksum(first, algorithm).await?.value
        == rd_files::compute_checksum(second, algorithm).await?.value)
}

/// Whether a file is there. A path below something that is not a folder holds nothing either.
pub(super) async fn exists(path: &Path) -> Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if absent(&error) => Ok(false),
        Err(error) => Err(error).with_context(|| format!("look for {}", path.display())),
    }
}

/// The answers that mean "nothing under that name".
fn absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

pub(super) async fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    Ok(())
}

/// Removes one file; one that is not there is fine.
pub(super) async fn discard(path: &Path) -> Result<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if absent(&error) => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}
