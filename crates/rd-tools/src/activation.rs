//! Which installed version is the active one, written as a pointer file.
//!
//! `<data>/tools/<name>/active.json` names a version directory that sits next to it. A
//! pointer file rather than a symlink because Windows only creates symlinks with a privilege
//! or developer mode switched on, and a feature that works on one platform and silently does
//! not on another is worse than one that uses the same mechanism everywhere.
//!
//! Rewriting it is a write-to-temporary plus `rename`, so a reader either sees the old
//! pointer or the new one and never a truncated file. The pointer is deliberately *not* the
//! record of what is installed — that is the directory listing and the database history — so
//! a lost or corrupt pointer costs an activation, not the installed versions.

use std::path::Path;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{error::ToolError, store::validate_segment};

/// File name of the pointer inside a tool's directory.
pub const POINTER_FILE_NAME: &str = "active.json";

/// The active version of one tool.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ActivePointer {
    /// Directory name under the tool's directory.
    pub version: String,
    /// When this version was activated, for the status display.
    pub activated_at: DateTime<Utc>,
}

/// Reads the pointer, or `None` when there is none or it cannot be read.
///
/// An unreadable pointer reads as "nothing activated" on purpose: the fallback is the vendor
/// folders and `PATH`, which is the behaviour this installation had before it managed
/// anything, and that is the right place to land. So does a version that is not a plain
/// directory name: the caller joins it onto the tool's directory, and the store's rule that a
/// string never becomes a path traversal holds for this file too (audit 2026-10-08, TR-08).
pub async fn read(tool_directory: &Path) -> Option<ActivePointer> {
    let bytes = tokio::fs::read(tool_directory.join(POINTER_FILE_NAME))
        .await
        .ok()?;
    let pointer: ActivePointer = serde_json::from_slice(&bytes).ok()?;
    if let Err(error) = validate_segment(&pointer.version) {
        tracing::warn!(%error, "the active-version pointer names no usable version");
        return None;
    }
    Some(pointer)
}

/// Points `tool_directory` at `version`, atomically.
pub async fn write(tool_directory: &Path, version: &str) -> Result<ActivePointer, ToolError> {
    let pointer = ActivePointer {
        version: version.to_owned(),
        activated_at: Utc::now(),
    };
    let encoded = serde_json::to_vec_pretty(&pointer)
        .context("serialise the active-version pointer")
        .map_err(ToolError::Other)?;
    let temporary = tool_directory.join(format!("{POINTER_FILE_NAME}.new"));
    let destination = tool_directory.join(POINTER_FILE_NAME);
    tokio::fs::create_dir_all(tool_directory)
        .await
        .with_context(|| format!("create {}", tool_directory.display()))
        .map_err(ToolError::Other)?;
    tokio::fs::write(&temporary, &encoded)
        .await
        .with_context(|| format!("write {}", temporary.display()))
        .map_err(ToolError::Other)?;
    if let Err(error) = tokio::fs::rename(&temporary, &destination).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(ToolError::Other(
            anyhow::Error::new(error).context(format!("activate {}", destination.display())),
        ));
    }
    Ok(pointer)
}

/// Removes the pointer, leaving every installed version in place.
pub async fn clear(tool_directory: &Path) {
    let _ = tokio::fs::remove_file(tool_directory.join(POINTER_FILE_NAME)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_written_pointer_reads_back_and_a_rewrite_replaces_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(read(directory.path()).await.is_none());
        write(directory.path(), "2024.01.01").await.expect("write");
        assert_eq!(
            read(directory.path()).await.expect("pointer").version,
            "2024.01.01"
        );
        write(directory.path(), "2024.09.07").await.expect("write");
        assert_eq!(
            read(directory.path()).await.expect("pointer").version,
            "2024.09.07"
        );
        clear(directory.path()).await;
        assert!(read(directory.path()).await.is_none());
    }

    /// A pointer that cannot be parsed must fall back to "nothing managed", not to an error
    /// that would take every tool lookup down with it.
    #[tokio::test]
    async fn an_unreadable_pointer_reads_as_nothing_activated() {
        let directory = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(directory.path().join(POINTER_FILE_NAME), b"{ not json")
            .await
            .expect("write");
        assert!(read(directory.path()).await.is_none());
    }

    #[tokio::test]
    async fn a_pointer_that_leaves_its_directory_reads_as_nothing_activated() {
        let directory = tempfile::tempdir().expect("tempdir");
        for version in ["../../bin", "/usr/bin", ".hidden", ""] {
            let pointer = serde_json::json!({
                "version": version,
                "activated_at": "2026-10-08T00:00:00Z",
            });
            tokio::fs::write(
                directory.path().join(POINTER_FILE_NAME),
                pointer.to_string(),
            )
            .await
            .expect("write");
            assert!(read(directory.path()).await.is_none(), "{version:?}");
        }
    }
}
