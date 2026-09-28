//! Replacing a verified duplicate by a link to its original (RD-150-02).
//!
//! Only on request, and only after proof: the two files are hashed, a hard link to the
//! original is created under a temporary name beside the duplicate, and that link is renamed
//! over the duplicate in one step. The duplicate's name never points at nothing, and a file
//! system that cannot link — another device, a FAT volume, a share that refuses — refuses before
//! anything was replaced.
//!
//! Reflinks (copy-on-write clones) are not offered. Creating one needs an `ioctl` on Linux and
//! `clonefile` on macOS, and neither is reachable without `unsafe` code or a new dependency;
//! [`LinkSupport::reflink`] therefore always says no, so the interface never offers what this
//! build cannot do.

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use rd_core::ChecksumAlgorithm;

use crate::compute_checksum;

/// What a directory's file system can do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkSupport {
    pub hardlink: bool,
    /// Always `false` in this build; see the module documentation.
    pub reflink: bool,
}

/// Why a duplicate was not linked. The duplicate is untouched in every case.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// The two files differ, in size or in bytes.
    #[error("{} and {} do not hold the same bytes", .0.display(), .1.display())]
    ContentDiffers(PathBuf, PathBuf),
    /// The two names already point at the same file.
    #[error("{} is already linked to {}", .1.display(), .0.display())]
    AlreadyLinked(PathBuf, PathBuf),
    /// The file system cannot link these two: another device, or no hard links at all.
    #[error("{} cannot be linked to {}: {}", .1.display(), .0.display(), .2)]
    Unsupported(PathBuf, PathBuf, #[source] std::io::Error),
    #[error("link {}: {}", .0.display(), .1)]
    Failed(PathBuf, anyhow::Error),
}

impl LinkError {
    /// A stable code for the history and the interface.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ContentDiffers(..) => "storage.dedupe_content_differs",
            Self::AlreadyLinked(..) => "storage.dedupe_already_linked",
            Self::Unsupported(..) => "storage.dedupe_link_unsupported",
            Self::Failed(..) => "storage.dedupe_failed",
        }
    }
}

/// What a completed link replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedDuplicate {
    /// SHA-256 both files were verified to share.
    pub digest: String,
    /// The bytes the duplicate occupied on its own.
    pub size_bytes: u64,
}

/// Probes whether hard links can be created inside `directory`.
///
/// Answered by trying: a small file and a link to it, both removed again. A share whose server
/// accepts the call but not the link fails here rather than halfway through a replacement.
pub async fn probe_link_support(directory: &Path) -> LinkSupport {
    let probe = directory.join(format!(".rdownloader-link-probe-{}", uuid::Uuid::now_v7()));
    let linked = probe.with_extension("link");
    let hardlink = async {
        tokio::fs::write(&probe, b"probe").await.ok()?;
        tokio::fs::hard_link(&probe, &linked).await.ok()
    }
    .await
    .is_some();
    for path in [&probe, &linked] {
        let _ = tokio::fs::remove_file(path).await;
    }
    LinkSupport {
        hardlink,
        reflink: false,
    }
}

/// Whether two existing paths are on one file system, where the platform can say.
///
/// `None` on platforms whose standard library does not expose a device number (Windows): the
/// link attempt is then the test, and it refuses before anything is replaced.
#[must_use]
pub fn same_file_system(first: &Path, second: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let first = std::fs::metadata(first).ok()?;
        let second = std::fs::metadata(second).ok()?;
        Some(first.dev() == second.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = (first, second);
        None
    }
}

/// Whether two paths already name the same file, where the platform can say.
fn already_linked(first: &std::fs::Metadata, second: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        first.dev() == second.dev() && first.ino() == second.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (first, second);
        false
    }
}

/// Replaces `duplicate` by a hard link to `original`, after proving both hold the same bytes.
pub async fn link_duplicate(
    original: &Path,
    duplicate: &Path,
) -> Result<LinkedDuplicate, LinkError> {
    let failed = |reason: anyhow::Error| LinkError::Failed(duplicate.to_path_buf(), reason);
    let original_meta = tokio::fs::metadata(original)
        .await
        .map_err(|error| failed(error.into()))?;
    let duplicate_meta = tokio::fs::metadata(duplicate)
        .await
        .map_err(|error| failed(error.into()))?;
    if !original_meta.is_file() || !duplicate_meta.is_file() {
        return Err(failed(anyhow::anyhow!("only regular files are linked")));
    }
    if already_linked(&original_meta, &duplicate_meta) {
        return Err(LinkError::AlreadyLinked(
            original.to_path_buf(),
            duplicate.to_path_buf(),
        ));
    }
    let differs = || LinkError::ContentDiffers(original.to_path_buf(), duplicate.to_path_buf());
    if original_meta.len() != duplicate_meta.len() {
        return Err(differs());
    }
    let digest = compute_checksum(original, ChecksumAlgorithm::Sha256)
        .await
        .map_err(failed)?
        .value;
    let other = compute_checksum(duplicate, ChecksumAlgorithm::Sha256)
        .await
        .map_err(failed)?
        .value;
    if digest != other {
        return Err(differs());
    }
    let name = duplicate
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = duplicate.with_file_name(format!(".{name}.rdlink"));
    let _ = tokio::fs::remove_file(&temporary).await;
    if let Err(error) = tokio::fs::hard_link(original, &temporary).await {
        return Err(if error.kind() == ErrorKind::NotFound {
            failed(error.into())
        } else {
            LinkError::Unsupported(original.to_path_buf(), duplicate.to_path_buf(), error)
        });
    }
    if let Err(error) = tokio::fs::rename(&temporary, duplicate).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(failed(error.into()));
    }
    Ok(LinkedDuplicate {
        digest,
        size_bytes: duplicate_meta.len(),
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::{LinkError, same_file_system};
    use super::{link_duplicate, probe_link_support};

    #[tokio::test]
    async fn a_temporary_directory_supports_hard_links_and_no_reflinks() {
        let directory = tempfile::tempdir().expect("tempdir");
        let support = probe_link_support(directory.path()).await;
        assert!(support.hardlink);
        assert!(!support.reflink, "this build offers no reflinks");
        let mut entries = std::fs::read_dir(directory.path()).expect("list");
        assert!(entries.next().is_none(), "the probe leaves nothing behind");
    }

    #[tokio::test]
    async fn an_identical_duplicate_becomes_a_link_to_the_original() {
        let directory = tempfile::tempdir().expect("tempdir");
        let original = directory.path().join("original.bin");
        let duplicate = directory.path().join("duplicate.bin");
        tokio::fs::write(&original, b"same bytes")
            .await
            .expect("write");
        tokio::fs::write(&duplicate, b"same bytes")
            .await
            .expect("write");

        let linked = link_duplicate(&original, &duplicate).await.expect("link");

        assert_eq!(linked.size_bytes, 10);
        assert_eq!(
            tokio::fs::read(&duplicate).await.expect("read"),
            b"same bytes"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let first = std::fs::metadata(&original).expect("meta");
            let second = std::fs::metadata(&duplicate).expect("meta");
            assert_eq!(first.ino(), second.ino(), "both names are one file now");
            assert_eq!(same_file_system(&original, &duplicate), Some(true));
        }
        // A second request finds nothing left to do.
        #[cfg(unix)]
        assert!(matches!(
            link_duplicate(&original, &duplicate).await,
            Err(LinkError::AlreadyLinked(..))
        ));
    }

    #[tokio::test]
    async fn a_file_with_other_bytes_is_never_replaced() {
        let directory = tempfile::tempdir().expect("tempdir");
        let original = directory.path().join("original.bin");
        let duplicate = directory.path().join("duplicate.bin");
        tokio::fs::write(&original, b"same length")
            .await
            .expect("write");
        tokio::fs::write(&duplicate, b"SAME length")
            .await
            .expect("write");

        let error = link_duplicate(&original, &duplicate)
            .await
            .expect_err("different bytes");

        assert_eq!(error.code(), "storage.dedupe_content_differs");
        assert_eq!(
            tokio::fs::read(&duplicate).await.expect("read"),
            b"SAME length"
        );
        let names = std::fs::read_dir(directory.path()).expect("list").count();
        assert_eq!(names, 2, "no temporary link is left behind");
    }
}
