//! Moving a file between directories that may live on different storage roots.

use std::{io::ErrorKind, path::Path};

use anyhow::{Context, Result};

/// Moves `from` to `to`, falling back to copy-and-remove across filesystem boundaries.
///
/// `rename` refuses a cross-device move with `EXDEV`, and two storage roots are routinely two
/// disks — a category that lives on the NAS and one that lives on the local SSD. The caller is
/// responsible for creating the target directory.
pub async fn move_file(from: &Path, to: &Path) -> Result<()> {
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device(&error) => copy_and_remove(from, to).await,
        Err(error) => {
            Err(error).with_context(|| format!("move {} to {}", from.display(), to.display()))
        }
    }
}

/// Moves a directory with its whole subtree, falling back to a recursive copy across devices.
///
/// Same contract as [`move_file`]: the caller creates the parent of `to`, and a cross-device
/// move copies before it removes, so a failure leaves the source intact.
pub async fn move_directory(from: &Path, to: &Path) -> Result<()> {
    match tokio::fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device(&error) => {
            copy_tree(from, to).await?;
            tokio::fs::remove_dir_all(from)
                .await
                .with_context(|| format!("remove {} after copying it", from.display()))
        }
        Err(error) => {
            Err(error).with_context(|| format!("move {} to {}", from.display(), to.display()))
        }
    }
}

/// Copies a directory tree entry by entry.
///
/// Iterative rather than recursive: an async function cannot call itself without boxing, and a
/// deeply nested release would otherwise be a stack-depth question.
async fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let mut pending = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, target)) = pending.pop() {
        tokio::fs::create_dir_all(&target)
            .await
            .with_context(|| format!("create {}", target.display()))?;
        let mut entries = tokio::fs::read_dir(&source)
            .await
            .with_context(|| format!("read {}", source.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let child_source = entry.path();
            let child_target = target.join(entry.file_name());
            if entry.file_type().await?.is_dir() {
                pending.push((child_source, child_target));
            } else {
                tokio::fs::copy(&child_source, &child_target)
                    .await
                    .with_context(|| {
                        format!(
                            "copy {} to {}",
                            child_source.display(),
                            child_target.display()
                        )
                    })?;
            }
        }
    }
    Ok(())
}

/// The cross-device fallback.
///
/// A failed copy leaves no half-written file behind, and the source is never dropped until its
/// copy is complete — a retry therefore starts from the same state as the first attempt.
async fn copy_and_remove(from: &Path, to: &Path) -> Result<()> {
    if let Err(error) = tokio::fs::copy(from, to).await {
        if let Err(cleanup) = tokio::fs::remove_file(to).await
            && cleanup.kind() != ErrorKind::NotFound
        {
            tracing::warn!(
                path = %to.display(),
                error = %cleanup,
                "partial copy was left behind after a failed move"
            );
        }
        return Err(error).with_context(|| format!("copy {} to {}", from.display(), to.display()));
    }
    tokio::fs::remove_file(from)
        .await
        .with_context(|| format!("remove {} after copying it", from.display()))
}

/// `rename` across filesystems: `EXDEV` on Unix, `ERROR_NOT_SAME_DEVICE` on Windows.
///
/// `ErrorKind::CrossesDevices` covers both, but only where the standard library recognises the
/// raw code; the numeric fallback keeps the detection working if it does not.
///
/// The fallback is per platform on purpose. The two numbers are not a shared vocabulary: 18 is
/// `EXDEV` on Unix and `ERROR_NO_MORE_FILES` on Windows, 17 is `ERROR_NOT_SAME_DEVICE` on
/// Windows and `EEXIST` on Unix. Accepting both everywhere meant a Unix `rename` refused with
/// `EEXIST` fell into the copy-then-delete path, which merges a directory into an existing one
/// and then removes the source — a destructive answer to an error that is not cross-device.
fn is_cross_device(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    const RAW_CROSS_DEVICE: i32 = 18; // EXDEV
    #[cfg(windows)]
    const RAW_CROSS_DEVICE: i32 = 17; // ERROR_NOT_SAME_DEVICE

    if error.kind() == ErrorKind::CrossesDevices {
        return true;
    }
    #[cfg(any(unix, windows))]
    {
        error.raw_os_error() == Some(RAW_CROSS_DEVICE)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use super::{copy_and_remove, is_cross_device, move_file};

    #[tokio::test]
    async fn a_move_within_one_filesystem_carries_the_content_over() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = temporary.path().join("file.bin");
        let target = temporary.path().join("moved.bin");
        tokio::fs::write(&source, b"payload").await.expect("write");

        move_file(&source, &target).await.expect("move");

        assert!(!source.exists(), "the source is gone after a move");
        assert_eq!(
            tokio::fs::read(&target).await.expect("read"),
            b"payload",
            "the content arrives unchanged"
        );
    }

    #[tokio::test]
    async fn the_cross_device_fallback_copies_before_it_removes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = temporary.path().join("file.bin");
        let target = temporary.path().join("copied.bin");
        tokio::fs::write(&source, b"payload").await.expect("write");

        copy_and_remove(&source, &target).await.expect("fallback");

        assert!(!source.exists(), "the source is removed once copied");
        assert_eq!(
            tokio::fs::read(&target).await.expect("read"),
            b"payload",
            "the copy holds the original content"
        );
    }

    #[tokio::test]
    async fn a_failed_copy_keeps_the_source_and_leaves_no_partial_target() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = temporary.path().join("missing.bin");
        let target = temporary.path().join("target.bin");

        copy_and_remove(&source, &target)
            .await
            .expect_err("copying a file that is not there must fail");

        assert!(!target.exists(), "no partial target survives a failed copy");
    }

    /// The raw fallback is the platform's own code, never the other platform's.
    ///
    /// This test used to accept 17 and 18 everywhere. On Unix 17 is `EEXIST`, so a `rename`
    /// refused because the target already exists was read as cross-device and answered with
    /// copy-then-`remove_dir_all(source)` — a destructive fallback for an error that is not
    /// cross-device at all.
    #[test]
    fn only_this_platforms_cross_device_code_triggers_the_fallback() {
        #[cfg(unix)]
        {
            assert!(is_cross_device(&std::io::Error::from_raw_os_error(18)));
            assert!(
                !is_cross_device(&std::io::Error::from_raw_os_error(17)),
                "EEXIST is not a cross-device rename"
            );
        }
        #[cfg(windows)]
        {
            assert!(is_cross_device(&std::io::Error::from_raw_os_error(17)));
            assert!(
                !is_cross_device(&std::io::Error::from_raw_os_error(18)),
                "ERROR_NO_MORE_FILES is not a cross-device rename"
            );
        }
        assert!(!is_cross_device(&std::io::Error::from(ErrorKind::NotFound)));
        assert!(!is_cross_device(&std::io::Error::from(
            ErrorKind::PermissionDenied
        )));
    }
}
