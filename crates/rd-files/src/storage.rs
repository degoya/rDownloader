use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use rd_core::StorageRootId;

/// An allowlisted destination root.
#[derive(Clone, Debug)]
pub struct StorageRoot {
    pub id: StorageRootId,
    pub name: String,
    path: PathBuf,
}

/// Why a path cannot serve as a storage root.
///
/// The reason is carried as a value rather than as message text because the API turns each
/// variant into its own stable error code for the interface to translate. Before this existed,
/// every one of these cases arrived as an untyped failure and left the service as
/// "internal service error" — which is neither true nor actionable: a download folder the user
/// cannot write to is the user's to fix, and they have to be told which part is wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageRootProblem {
    /// A relative path; a root has to be absolute or it moves with the working directory.
    NotAbsolute,
    /// Something is already there and it is not a directory.
    PathIsFile,
    /// The directory cannot be created or written where it is.
    PermissionDenied,
    /// The filesystem underneath is mounted read-only.
    ReadOnlyFilesystem,
    /// Creating it failed for a reason none of the above names.
    NotCreatable,
    /// It exists and could be created, but nothing can be written into it.
    NotWritable,
}

impl StorageRootProblem {
    /// The stable error code the API reports this as.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::NotAbsolute => "storage_root.path_not_absolute",
            Self::PathIsFile => "storage_root.path_is_file",
            Self::PermissionDenied => "storage_root.permission_denied",
            Self::ReadOnlyFilesystem => "storage_root.read_only_filesystem",
            Self::NotCreatable => "storage_root.not_creatable",
            Self::NotWritable => "storage_root.not_writable",
        }
    }

    /// English fallback for clients that do not translate the code.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAbsolute => "Storage root path must be absolute",
            Self::PathIsFile => "A file already exists at this path",
            Self::PermissionDenied => "No permission to create or write in this location",
            Self::ReadOnlyFilesystem => "This location is on a read-only filesystem",
            Self::NotCreatable => "This directory could not be created",
            Self::NotWritable => "This directory exists but cannot be written to",
        }
    }
}

/// Checks that `path` can serve as a storage root, creating the directory if it is missing.
///
/// Creating the directory is not enough on its own to know it is usable — a directory owned by
/// somebody else, a read-only mount or an ACL all let `create_dir_all` succeed on an existing
/// path and refuse the first download hours later. So this writes a probe file and removes it
/// again, which is the only thing that actually answers the question.
///
/// Runs before `StorageRoot::create` rather than inside it: ten call sites on the download paths
/// take a root that was already accepted, and none of them should pay for a probe write.
pub async fn ensure_usable(path: &Path) -> std::result::Result<(), StorageRootProblem> {
    if !path.is_absolute() {
        return Err(StorageRootProblem::NotAbsolute);
    }
    match tokio::fs::metadata(path).await {
        Ok(metadata) if !metadata.is_dir() => return Err(StorageRootProblem::PathIsFile),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(classify(&error)),
    }
    if let Err(error) = tokio::fs::create_dir_all(path).await {
        return Err(classify(&error));
    }
    probe_write(path).await
}

/// Writes a file into `path` and removes it again.
///
/// The name carries the process id so two services sharing a folder cannot collide, and the
/// file is removed even when the write itself succeeded but the removal is what fails — a
/// leftover probe file in the user's download folder would be its own small bug.
async fn probe_write(path: &Path) -> std::result::Result<(), StorageRootProblem> {
    let probe = path.join(format!(".rdownloader-write-probe-{}", std::process::id()));
    let outcome = tokio::fs::write(&probe, b"").await;
    let removal = tokio::fs::remove_file(&probe).await;
    match outcome {
        Ok(()) => {
            // A write that lands but cannot be cleaned up still proves the folder is writable.
            drop(removal);
            Ok(())
        }
        Err(error) => Err(match classify(&error) {
            StorageRootProblem::NotCreatable => StorageRootProblem::NotWritable,
            named => named,
        }),
    }
}

fn classify(error: &std::io::Error) -> StorageRootProblem {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => StorageRootProblem::PermissionDenied,
        std::io::ErrorKind::ReadOnlyFilesystem => StorageRootProblem::ReadOnlyFilesystem,
        std::io::ErrorKind::NotADirectory | std::io::ErrorKind::AlreadyExists => {
            StorageRootProblem::PathIsFile
        }
        _ => StorageRootProblem::NotCreatable,
    }
}

impl StorageRoot {
    /// Creates and canonicalizes a root directory.
    pub async fn create(id: StorageRootId, name: String, path: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&path)
            .await
            .with_context(|| format!("create storage root {}", path.display()))?;
        let path = dunce::canonicalize(&path)
            .with_context(|| format!("canonicalize storage root {}", path.display()))?;
        Ok(Self { id, name, path })
    }

    /// Opens an existing directory as a root without creating it; `None` when it is gone.
    ///
    /// Cleanup paths need the allowlist semantics of a root but must not resurrect a
    /// directory the user has moved away.
    pub async fn open_existing(
        id: StorageRootId,
        name: String,
        path: PathBuf,
    ) -> Result<Option<Self>> {
        let canonical = match dunce::canonicalize(&path) {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(anyhow::Error::from(error))
                    .with_context(|| format!("canonicalize storage root {}", path.display()));
            }
        };
        Ok(Some(Self {
            id,
            name,
            path: canonical,
        }))
    }

    /// Returns the canonical root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolves a relative path while rejecting traversal and symlink escapes.
    pub fn resolve(&self, relative: &Path) -> Result<PathBuf> {
        if relative.as_os_str().is_empty() {
            return Ok(self.path.clone());
        }
        if relative.is_absolute() {
            bail!("destination must be relative to its storage root");
        }

        let mut resolved = self.path.clone();
        for component in relative.components() {
            let Component::Normal(segment) = component else {
                bail!("destination contains a forbidden path component");
            };
            resolved.push(segment);
            if resolved.exists() {
                resolved = dunce::canonicalize(&resolved)
                    .with_context(|| format!("canonicalize {}", resolved.display()))?;
                if !resolved.starts_with(&self.path) {
                    bail!("destination escapes its storage root through a symlink");
                }
            }
        }

        if !resolved.starts_with(&self.path) {
            bail!("destination escapes its storage root");
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rd_core::StorageRootId;

    use super::StorageRoot;

    #[tokio::test]
    async fn blocks_parent_components() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = StorageRoot::create(
            StorageRootId::new(),
            "test".to_owned(),
            directory.path().to_owned(),
        )
        .await
        .expect("root");
        assert!(root.resolve(Path::new("../escape")).is_err());
        assert!(root.resolve(Path::new("movies/example")).is_ok());
    }

    #[tokio::test]
    async fn opening_a_missing_directory_does_not_create_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let missing = directory.path().join("moved-away");
        let root =
            StorageRoot::open_existing(StorageRootId::new(), "test".to_owned(), missing.clone())
                .await
                .expect("open");
        assert!(root.is_none(), "a missing directory has no root");
        assert!(!missing.exists(), "opening must not create the directory");
    }

    #[tokio::test]
    async fn opens_an_existing_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let root = StorageRoot::open_existing(
            StorageRootId::new(),
            "test".to_owned(),
            directory.path().to_owned(),
        )
        .await
        .expect("open")
        .expect("existing root");
        assert_eq!(
            root.path(),
            dunce::canonicalize(directory.path())
                .expect("canonicalize")
                .as_path()
        );
    }

    #[tokio::test]
    async fn a_relative_path_is_named_as_such() {
        assert_eq!(
            super::ensure_usable(Path::new("downloads")).await,
            Err(super::StorageRootProblem::NotAbsolute)
        );
    }

    #[tokio::test]
    async fn a_missing_directory_is_created_and_accepted() {
        let directory = tempfile::tempdir().expect("tempdir");
        let target = directory.path().join("nested/downloads");
        super::ensure_usable(&target).await.expect("usable");
        assert!(target.is_dir());
    }

    #[tokio::test]
    async fn a_path_that_is_a_file_is_refused() {
        let directory = tempfile::tempdir().expect("tempdir");
        let target = directory.path().join("not-a-directory");
        std::fs::write(&target, b"x").expect("write");
        assert_eq!(
            super::ensure_usable(&target).await,
            Err(super::StorageRootProblem::PathIsFile)
        );
    }

    /// The reported case: `/downloads` sits in a directory owned by root, so `create_dir_all`
    /// fails with `PermissionDenied` and the service answered "internal service error".
    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_that_cannot_be_created_names_the_permission() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("tempdir");
        let parent = directory.path().join("locked");
        std::fs::create_dir(&parent).expect("parent");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        let outcome = super::ensure_usable(&parent.join("downloads")).await;
        // Restore before asserting so the temporary directory can always be cleaned up.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        assert_eq!(outcome, Err(super::StorageRootProblem::PermissionDenied));
    }

    /// Creating a directory proves nothing about writing into it. A folder that already exists
    /// and is read-only passes `create_dir_all` and fails at the first download instead.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_existing_directory_that_cannot_be_written_to_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("tempdir");
        let target = directory.path().join("read-only");
        std::fs::create_dir(&target).expect("dir");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        let outcome = super::ensure_usable(&target).await;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        assert_eq!(outcome, Err(super::StorageRootProblem::PermissionDenied));
    }

    #[tokio::test]
    async fn the_probe_file_does_not_survive_the_check() {
        let directory = tempfile::tempdir().expect("tempdir");
        super::ensure_usable(directory.path())
            .await
            .expect("usable");
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .expect("read")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }

    #[test]
    fn every_problem_has_its_own_code() {
        let problems = [
            super::StorageRootProblem::NotAbsolute,
            super::StorageRootProblem::PathIsFile,
            super::StorageRootProblem::PermissionDenied,
            super::StorageRootProblem::ReadOnlyFilesystem,
            super::StorageRootProblem::NotCreatable,
            super::StorageRootProblem::NotWritable,
        ];
        let codes: std::collections::BTreeSet<_> =
            problems.iter().map(|problem| problem.code()).collect();
        assert_eq!(codes.len(), problems.len(), "codes have to be distinct");
        assert!(codes.iter().all(|code| code.starts_with("storage_root.")));
    }
}
