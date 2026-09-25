//! The on-disk layout of managed tool versions, and the atomic moves that change it.
//!
//! `<data>/tools/<name>/<version>/` holds the binaries of one build; nothing is ever written
//! into a version directory that already exists. An install writes into a sibling staging
//! directory named `.install-<uuid>` — a sibling so the promoting `rename` stays inside one
//! filesystem, where it is atomic — and only then renames it into place. Every failure path
//! removes the staging directory, so a cancelled or broken install leaves the store exactly
//! as it found it rather than leaving a half-written version that looks installed.
//!
//! This mirrors `rd_plugin_host::PluginInstaller::install_verified`, and for the same reason:
//! a partially written artefact that the resolver can find is worse than no artefact at all.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::error::ToolError;

/// The directory name holding every managed tool, under the data directory.
pub const TOOLS_DIR_NAME: &str = "tools";

/// How many versions of one tool are kept after an activation.
///
/// The active one, the one before it (which is what a rollback reaches for), and one more so
/// that two activations in a row do not throw away the version somebody actually wanted back.
pub const KEPT_VERSIONS_PER_TOOL: usize = 3;

/// Refuses a name or version that would escape its directory.
///
/// A manifest is signed, but a signature is not a reason to let a string become a path
/// traversal: the key could rotate, the publisher could be compromised, and the cost of
/// checking is nothing.
pub fn validate_segment(segment: &str) -> Result<(), ToolError> {
    let acceptable = !segment.is_empty()
        && segment.len() <= 64
        && segment != "."
        && segment != ".."
        && !segment.starts_with('.')
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'));
    if acceptable {
        Ok(())
    } else {
        Err(ToolError::Other(anyhow::anyhow!(
            "{segment:?} is not usable as a directory name"
        )))
    }
}

/// The managed tool store rooted at one directory.
#[derive(Clone, Debug)]
pub struct ToolStore {
    root: PathBuf,
}

impl ToolStore {
    /// A store under `root`, which is created lazily on the first install.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The directory holding every version of every managed tool.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/<name>`.
    pub fn tool_directory(&self, name: &str) -> Result<PathBuf, ToolError> {
        validate_segment(name)?;
        Ok(self.root.join(name))
    }

    /// `<root>/<name>/<version>`.
    pub fn version_directory(&self, name: &str, version: &str) -> Result<PathBuf, ToolError> {
        validate_segment(version)?;
        Ok(self.tool_directory(name)?.join(version))
    }

    /// Every version of `name` present on disk, unordered. Missing directories read as empty
    /// rather than as an error: a tool nobody has installed is a normal state.
    pub async fn installed_versions(&self, name: &str) -> Result<Vec<String>, ToolError> {
        let directory = self.tool_directory(name)?;
        let mut versions = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
            return Ok(versions);
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_name = entry.file_name();
            let Some(text) = file_name.to_str() else {
                continue;
            };
            // Skips `.install-…` leftovers and the `active.json` pointer in one rule.
            if validate_segment(text).is_err() {
                continue;
            }
            if entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                versions.push(text.to_owned());
            }
        }
        Ok(versions)
    }

    /// Whether one exact version is on disk.
    pub async fn is_installed(&self, name: &str, version: &str) -> bool {
        match self.version_directory(name, version) {
            Ok(path) => tokio::fs::metadata(&path).await.is_ok(),
            Err(_) => false,
        }
    }

    /// Creates a staging directory next to where the version will land.
    ///
    /// A sibling, not a temporary directory elsewhere: the promoting `rename` has to stay on
    /// one filesystem to be atomic, and `/tmp` routinely is not the same filesystem.
    pub async fn stage(&self, name: &str) -> Result<Staging, ToolError> {
        let tool_directory = self.tool_directory(name)?;
        tokio::fs::create_dir_all(&tool_directory)
            .await
            .with_context(|| format!("create {}", tool_directory.display()))
            .map_err(ToolError::Other)?;
        let path = tool_directory.join(format!(".install-{}", uuid::Uuid::now_v7()));
        tokio::fs::create_dir(&path)
            .await
            .with_context(|| format!("create staging directory {}", path.display()))
            .map_err(ToolError::Other)?;
        Ok(Staging { path })
    }

    /// Moves a staging directory into place as `<name>/<version>`.
    ///
    /// Refuses an already installed version instead of replacing it: an install that
    /// overwrites what a running job is executing is the failure mode this whole layout
    /// exists to avoid.
    pub async fn promote(
        &self,
        staging: Staging,
        name: &str,
        version: &str,
    ) -> Result<PathBuf, ToolError> {
        let destination = self.version_directory(name, version)?;
        if tokio::fs::metadata(&destination).await.is_ok() {
            staging.discard().await;
            return Err(ToolError::Other(anyhow::anyhow!(
                "{name} {version} is already installed"
            )));
        }
        let source = staging.take();
        if let Err(error) = tokio::fs::rename(&source, &destination).await {
            let _ = tokio::fs::remove_dir_all(&source).await;
            return Err(ToolError::Other(
                anyhow::Error::new(error).context(format!("activate {}", destination.display())),
            ));
        }
        Ok(destination)
    }

    /// Removes one installed version, refusing any path outside this store.
    pub async fn remove_version(&self, name: &str, version: &str) -> Result<(), ToolError> {
        let target = self.version_directory(name, version)?;
        self.remove_contained(&target)
            .await
            .map_err(ToolError::Other)
    }

    /// Deletes `path` only when it really sits under this store's root.
    ///
    /// Canonicalised on both sides, so a symlinked version directory cannot make a
    /// `remove_dir_all` land somewhere else.
    async fn remove_contained(&self, path: &Path) -> anyhow::Result<()> {
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !target.starts_with(&root) || target == root {
            bail!(
                "refusing to remove {} outside the tool store",
                path.display()
            );
        }
        tokio::fs::remove_dir_all(&target).await?;
        Ok(())
    }

    /// Removes staging directories a previous run left behind, best effort.
    ///
    /// A crash between `create_dir` and `rename` leaves one, and nothing else ever will:
    /// `.install-…` is not a name the resolver or the version listing accepts.
    pub async fn sweep_staging(&self, name: &str) {
        let Ok(directory) = self.tool_directory(name) else {
            return;
        };
        let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_name = entry.file_name();
            let Some(text) = file_name.to_str() else {
                continue;
            };
            if text.starts_with(".install-") {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }
}

/// A staging directory that removes itself unless it is promoted.
///
/// The `Drop` is a backstop for a panic or an early `?`; every ordinary path calls
/// [`Staging::discard`] or [`ToolStore::promote`] explicitly, because a blocking
/// `remove_dir_all` inside `Drop` is not something to rely on in an async runtime.
#[derive(Debug)]
pub struct Staging {
    path: PathBuf,
}

impl Staging {
    /// Where to write the files that will become the version directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes the staging directory and everything written into it.
    pub async fn discard(self) {
        let path = self.take();
        let _ = tokio::fs::remove_dir_all(&path).await;
    }

    /// Gives up ownership without deleting, for the promoting rename.
    fn take(mut self) -> PathBuf {
        std::mem::take(&mut self.path)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_traversing_segment_is_refused() {
        for segment in ["..", ".", "", "../etc", "a/b", ".hidden", "a b"] {
            assert!(validate_segment(segment).is_err(), "{segment:?}");
        }
    }

    #[test]
    fn an_ordinary_tool_version_is_accepted() {
        for segment in ["2024.09.07", "6.1.1", "yt-dlp", "1.0.0+build", "gallery-dl"] {
            assert!(validate_segment(segment).is_ok(), "{segment:?}");
        }
    }

    #[tokio::test]
    async fn a_discarded_staging_directory_leaves_nothing_behind() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = ToolStore::new(directory.path().join("tools"));
        let staging = store.stage("yt-dlp").await.expect("staging");
        let path = staging.path().to_path_buf();
        tokio::fs::write(path.join("yt-dlp"), b"payload")
            .await
            .expect("write");
        staging.discard().await;
        assert!(!path.exists());
        assert!(
            store
                .installed_versions("yt-dlp")
                .await
                .expect("versions")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn removing_a_path_outside_the_store_is_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = ToolStore::new(directory.path().join("tools"));
        let outside = directory.path().join("elsewhere");
        tokio::fs::create_dir_all(&outside).await.expect("create");
        assert!(store.remove_contained(&outside).await.is_err());
        assert!(outside.exists());
    }
}
