//! A move that proves its copy before it lets go of the original (RD-150-02).
//!
//! [`crate::move_file`] falls back to copy-and-remove across devices and trusts the copy the
//! moment `copy` returns. That is the first half of a verified move. This is the second: the
//! copy goes to a temporary name beside the target, both sides are hashed, and only a copy
//! whose SHA-256 equals the original's is renamed into place — atomically, inside the target
//! directory — before the original is removed.
//!
//! The protocol is split in two steps on purpose, [`place_verified`] and [`release_source`],
//! so that the instant between them is one a caller can name as a crash point. Every state a
//! stop can leave behind is one the next attempt recognises:
//!
//! | Stopped | Found by the next attempt | It does |
//! | --- | --- | --- |
//! | during the copy | original, a temporary file | drops the temporary file, copies again |
//! | after the rename into place | original and an identical target | verifies, removes the original |
//! | after the removal | the target alone | nothing is left to do |
//!
//! No step ever removes the last copy: the original goes only after the target was verified
//! to hold the same bytes, and a failed or mismatching copy removes the temporary file and
//! nothing else.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use rd_core::ChecksumAlgorithm;

use crate::compute_checksum;

/// Why a verified move did not happen. The original is untouched in every case.
#[derive(Debug, thiserror::Error)]
pub enum VerifiedMoveError {
    /// The target name holds a different file; the caller picks another name.
    #[error("{} already holds a different file", .0.display())]
    TargetTaken(PathBuf),
    /// The copy did not hash to the original's digest and was discarded.
    #[error("the copy of {} did not verify against the original", .0.display())]
    Mismatch(PathBuf),
    #[error("move {} to {}: {source}", from.display(), to.display())]
    Io {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("hash {}: {reason}", path.display())]
    Hash {
        path: PathBuf,
        reason: anyhow::Error,
    },
}

impl VerifiedMoveError {
    /// A stable code for the history and the interface.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TargetTaken(_) => "storage.move_target_taken",
            Self::Mismatch(_) => "storage.move_verification_failed",
            Self::Io { .. } | Self::Hash { .. } => "storage.move_failed",
        }
    }
}

/// Where the content is after [`place_verified`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedCopy {
    pub size_bytes: u64,
    /// SHA-256 both sides were verified to share; `None` when a rename within one device moved
    /// the file itself and there was nothing to compare.
    pub digest: Option<String>,
    /// Whether the original is still there and [`release_source`] has to remove it.
    pub source_remains: bool,
}

/// Puts the content of `from` at `to`, verified. The caller creates the target directory.
///
/// Within one device this is a rename. Across devices it copies to a temporary name, compares
/// the SHA-256 of both sides and renames the copy into place; the original stays until
/// [`release_source`]. A target that already holds the identical file — what a stop after the
/// rename leaves behind — is accepted as placed.
pub async fn place_verified(from: &Path, to: &Path) -> Result<PlacedCopy, VerifiedMoveError> {
    let io = |source: std::io::Error| VerifiedMoveError::Io {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        source,
    };
    let source_size = tokio::fs::metadata(from).await.map_err(io)?.len();
    if let Ok(existing) = tokio::fs::metadata(to).await {
        // The resume of a stopped move, or somebody else's file: only the bytes can tell.
        if existing.is_file() && existing.len() == source_size {
            let original = sha256(from).await?;
            if sha256(to).await? == original {
                return Ok(PlacedCopy {
                    size_bytes: source_size,
                    digest: Some(original),
                    source_remains: true,
                });
            }
        }
        return Err(VerifiedMoveError::TargetTaken(to.to_path_buf()));
    }
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(PlacedCopy {
            size_bytes: source_size,
            digest: None,
            source_remains: false,
        }),
        Err(error) if crate::moves::is_cross_device(&error) => copy_verified(from, to).await,
        Err(error) => Err(io(error)),
    }
}

/// Removes the original of a placed copy. Nothing to do when the rename already moved it.
pub async fn release_source(from: &Path, placed: &PlacedCopy) -> Result<(), VerifiedMoveError> {
    if !placed.source_remains {
        return Ok(());
    }
    match tokio::fs::remove_file(from).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(VerifiedMoveError::Io {
            from: from.to_path_buf(),
            to: from.to_path_buf(),
            source,
        }),
    }
}

/// Both steps at once, for a caller that has no crash point to put between them.
pub async fn verified_move_file(from: &Path, to: &Path) -> Result<PlacedCopy, VerifiedMoveError> {
    let placed = place_verified(from, to).await?;
    release_source(from, &placed).await?;
    Ok(placed)
}

/// The temporary name a copy is written under, beside its target.
fn temporary_of(to: &Path) -> PathBuf {
    let name = to
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    to.with_file_name(format!(".{name}.rdmove"))
}

/// The cross-device half: copy, sync, hash both sides, rename into place.
pub(crate) async fn copy_verified(from: &Path, to: &Path) -> Result<PlacedCopy, VerifiedMoveError> {
    let temporary = temporary_of(to);
    let io = |source: std::io::Error| VerifiedMoveError::Io {
        from: from.to_path_buf(),
        to: temporary.clone(),
        source,
    };
    // Whatever an earlier, stopped copy left under the temporary name is not trusted.
    discard(&temporary).await;
    let result = async {
        let size_bytes = tokio::fs::copy(from, &temporary).await.map_err(io)?;
        // Opened for writing: Windows flushes a file only through a handle with write access
        // and answers a read-only one with "Access is denied".
        tokio::fs::OpenOptions::new()
            .write(true)
            .open(&temporary)
            .await
            .map_err(io)?
            .sync_all()
            .await
            .map_err(io)?;
        let original = sha256(from).await?;
        if sha256(&temporary).await? != original {
            return Err(VerifiedMoveError::Mismatch(from.to_path_buf()));
        }
        tokio::fs::rename(&temporary, to).await.map_err(io)?;
        Ok(PlacedCopy {
            size_bytes,
            digest: Some(original),
            source_remains: true,
        })
    }
    .await;
    if result.is_err() {
        discard(&temporary).await;
    }
    result
}

async fn discard(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "a temporary move copy was left behind");
    }
}

async fn sha256(path: &Path) -> Result<String, VerifiedMoveError> {
    compute_checksum(path, ChecksumAlgorithm::Sha256)
        .await
        .map(|computed| computed.value)
        .map_err(|reason| VerifiedMoveError::Hash {
            path: path.to_path_buf(),
            reason,
        })
}

#[cfg(test)]
mod tests {
    use super::{
        PlacedCopy, VerifiedMoveError, copy_verified, place_verified, release_source, temporary_of,
        verified_move_file,
    };

    #[tokio::test]
    async fn a_move_within_one_device_renames_and_leaves_nothing_to_release() {
        let directory = tempfile::tempdir().expect("tempdir");
        let from = directory.path().join("a.bin");
        let to = directory.path().join("b.bin");
        tokio::fs::write(&from, b"payload").await.expect("write");

        let placed = verified_move_file(&from, &to).await.expect("move");

        assert!(!placed.source_remains);
        assert_eq!(placed.size_bytes, 7);
        assert!(!from.exists());
        assert_eq!(tokio::fs::read(&to).await.expect("read"), b"payload");
    }

    /// The cross-device path, driven directly: a test cannot make `rename` answer `EXDEV`.
    #[tokio::test]
    async fn a_copy_is_verified_before_the_original_is_released() {
        let directory = tempfile::tempdir().expect("tempdir");
        let from = directory.path().join("a.bin");
        let to = directory.path().join("b.bin");
        tokio::fs::write(&from, b"payload").await.expect("write");

        let placed = copy_verified(&from, &to).await.expect("copy");
        assert!(
            placed.source_remains,
            "the original stays until it is released"
        );
        assert!(from.exists() && to.exists());
        assert!(placed.digest.is_some());
        assert!(!temporary_of(&to).exists(), "the temporary name is gone");

        release_source(&from, &placed).await.expect("release");
        assert!(!from.exists());
        assert_eq!(tokio::fs::read(&to).await.expect("read"), b"payload");
    }

    /// A stop after the rename into place: both copies exist and are identical. The next
    /// attempt verifies instead of filing a second copy under another name.
    #[tokio::test]
    async fn an_identical_target_is_the_resume_of_a_stopped_move() {
        let directory = tempfile::tempdir().expect("tempdir");
        let from = directory.path().join("a.bin");
        let to = directory.path().join("b.bin");
        tokio::fs::write(&from, b"payload").await.expect("write");
        tokio::fs::write(&to, b"payload").await.expect("write");

        let placed = verified_move_file(&from, &to).await.expect("resume");

        assert!(placed.digest.is_some());
        assert!(!from.exists());
        assert_eq!(tokio::fs::read(&to).await.expect("read"), b"payload");
    }

    /// A stop during the copy: a truncated temporary file is never trusted and never kept.
    #[tokio::test]
    async fn a_leftover_temporary_copy_is_replaced() {
        let directory = tempfile::tempdir().expect("tempdir");
        let from = directory.path().join("a.bin");
        let to = directory.path().join("b.bin");
        tokio::fs::write(&from, b"payload").await.expect("write");
        tokio::fs::write(temporary_of(&to), b"pay")
            .await
            .expect("stale");

        let placed = copy_verified(&from, &to).await.expect("copy");

        assert_eq!(placed.size_bytes, 7);
        assert_eq!(tokio::fs::read(&to).await.expect("read"), b"payload");
        assert!(!temporary_of(&to).exists());
    }

    #[tokio::test]
    async fn a_different_file_under_the_target_name_is_never_touched() {
        let directory = tempfile::tempdir().expect("tempdir");
        let from = directory.path().join("a.bin");
        let to = directory.path().join("b.bin");
        tokio::fs::write(&from, b"payload").await.expect("write");
        tokio::fs::write(&to, b"another").await.expect("write");

        let error = place_verified(&from, &to).await.expect_err("taken");

        assert!(matches!(error, VerifiedMoveError::TargetTaken(_)));
        assert_eq!(error.code(), "storage.move_target_taken");
        assert_eq!(tokio::fs::read(&from).await.expect("read"), b"payload");
        assert_eq!(tokio::fs::read(&to).await.expect("read"), b"another");
    }

    #[tokio::test]
    async fn a_failed_copy_keeps_the_original_and_leaves_no_temporary_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let from = directory.path().join("missing.bin");
        let to = directory.path().join("b.bin");

        copy_verified(&from, &to)
            .await
            .expect_err("nothing to copy");

        assert!(!to.exists());
        assert!(!temporary_of(&to).exists());
    }

    #[tokio::test]
    async fn releasing_a_renamed_file_does_nothing() {
        let directory = tempfile::tempdir().expect("tempdir");
        let from = directory.path().join("gone.bin");
        release_source(
            &from,
            &PlacedCopy {
                size_bytes: 0,
                digest: None,
                source_remains: false,
            },
        )
        .await
        .expect("nothing to release");
    }
}
