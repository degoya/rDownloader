//! Where a finished archive goes (RD-160-01, RD-160-02).
//!
//! One small trait every destination meets the same way — the contract `tests/destinations.rs`
//! runs against each of them:
//!
//! * [`BackupDestination::store`] places a *copy*: the archive goes to several destinations,
//!   so the local file stays. Nothing appears under the name unless it is the whole archive,
//!   and a name that is taken is never replaced.
//! * [`BackupDestination::list`] names the archives (`*.rdbackup`) directly in the
//!   destination, whoever wrote them; everything else there is invisible to it.
//! * [`BackupDestination::fetch`] copies one back for a verification or a restore.
//! * [`BackupDestination::remove`] deletes one archive by name, for retention, which asks only
//!   about names this installation recorded (`crate::retention`).
//!
//! Every name is checked by [`is_archive_name`] first, so no call can reach outside the
//! destination or touch a file that is not an archive. The local folder is here; object
//! storage and rclone are in `crate::remote`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rd_files::{StorageRootProblem, VerifiedMoveError};

/// Where an archive ended up, in words the history shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredBackup {
    pub location: String,
}

/// An archive found in a destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListedArchive {
    pub name: String,
    pub size: u64,
}

/// Why a destination refused or failed; each variant has a stable code.
#[derive(Debug, thiserror::Error)]
pub enum DestinationError {
    /// The folder cannot be used: not absolute, not a directory, not writable.
    #[error("{}", .0.message())]
    Unusable(StorageRootProblem),
    /// The destination's configuration cannot work as it is: a deleted profile, no rclone.
    #[error("{detail}")]
    Misconfigured { code: &'static str, detail: String },
    /// The name is not a plain archive file name.
    #[error("the archive name {0} is not a plain archive file name")]
    InvalidName(String),
    /// Something is already there under the archive's name; it is never overwritten.
    #[error("an archive named {0} is already there")]
    NameTaken(String),
    /// No archive of that name is there.
    #[error("no archive named {0} is there")]
    NotFound(String),
    /// The service or the remote did not answer as it should; worth another attempt.
    #[error("{detail}")]
    Unavailable { code: &'static str, detail: String },
    /// Copying, comparing or renaming failed.
    #[error("{0}")]
    Failed(String),
}

impl DestinationError {
    /// The stable code the history and the API report.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unusable(problem) => problem.code(),
            Self::Misconfigured { code, .. } | Self::Unavailable { code, .. } => code,
            Self::InvalidName(_) => "backup.archive_name_invalid",
            Self::NameTaken(_) => "backup.destination_name_taken",
            Self::NotFound(_) => "backup.archive_not_found",
            Self::Failed(_) => "backup.destination_failed",
        }
    }

    /// Whether another attempt can go differently: an outage can end, a wrong path cannot.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(self, Self::Unavailable { .. } | Self::Failed(_))
    }
}

/// Whether `name` is a name an archive may have in a destination: one plain file name, ending
/// in `.rdbackup`, not hidden.
#[must_use]
pub fn is_archive_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && rd_files::sanitize_file_name(name) == name
        && name
            .strip_suffix(crate::ARCHIVE_EXTENSION)
            .is_some_and(|stem| stem.len() > 1 && stem.ends_with('.'))
}

/// [`is_archive_name`] as the error every call starts with.
///
/// # Errors
///
/// [`DestinationError::InvalidName`] for anything else.
pub fn check_archive_name(name: &str) -> Result<(), DestinationError> {
    if is_archive_name(name) {
        Ok(())
    } else {
        Err(DestinationError::InvalidName(name.to_owned()))
    }
}

/// A place archives are written to.
#[async_trait]
pub trait BackupDestination: Send + Sync {
    /// The kind stored in `backup_destinations.kind`.
    fn kind(&self) -> &'static str;

    /// How the history names this destination.
    fn describe(&self) -> String;

    /// Places a copy of the archive at `archive` under `name`; the local file stays. Nothing
    /// appears under `name` unless it is the whole archive, and a taken name is never replaced.
    async fn store(&self, archive: &Path, name: &str) -> Result<StoredBackup, DestinationError>;

    /// The archives directly in the destination, whoever wrote them.
    async fn list(&self) -> Result<Vec<ListedArchive>, DestinationError>;

    /// Copies the archive `name` to `into`, which must not exist yet; returns its size.
    async fn fetch(&self, name: &str, into: &Path) -> Result<u64, DestinationError>;

    /// Deletes the archive `name`.
    async fn remove(&self, name: &str) -> Result<(), DestinationError>;
}

/// A folder on this machine, a mounted NAS share included.
#[derive(Clone, Debug)]
pub struct LocalFolder {
    path: PathBuf,
}

impl LocalFolder {
    /// The kind this destination is stored as.
    pub const KIND: &'static str = "local";

    /// Checks the folder the way a storage root is checked — absolute, a directory, writable,
    /// created when missing — and opens it.
    ///
    /// # Errors
    ///
    /// When the folder cannot serve, with the storage root's reason.
    pub async fn open(path: &Path) -> Result<Self, DestinationError> {
        rd_files::ensure_usable(path)
            .await
            .map_err(DestinationError::Unusable)?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// The folder a stored `local` destination names, from its `config_json`.
    #[must_use]
    pub fn path_of(config: &serde_json::Value) -> Option<PathBuf> {
        config
            .get("path")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
    }

    /// The `config_json` of a `local` destination.
    #[must_use]
    pub fn config_of(path: &Path) -> serde_json::Value {
        serde_json::json!({ "path": path.display().to_string() })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn io_failure(error: &std::io::Error, name: &str) -> DestinationError {
    if error.kind() == std::io::ErrorKind::NotFound {
        DestinationError::NotFound(name.to_owned())
    } else {
        DestinationError::Failed(error.to_string())
    }
}

#[async_trait]
impl BackupDestination for LocalFolder {
    fn kind(&self) -> &'static str {
        Self::KIND
    }

    fn describe(&self) -> String {
        self.path.display().to_string()
    }

    async fn store(&self, archive: &Path, name: &str) -> Result<StoredBackup, DestinationError> {
        check_archive_name(name)?;
        let target = self.path.join(name);
        if tokio::fs::symlink_metadata(&target).await.is_ok() {
            return Err(DestinationError::NameTaken(name.to_owned()));
        }
        // A copy under a temporary name beside the target, both sides hashed, then the rename:
        // never a half-written file under the final name. The archive stays for the next
        // destination.
        rd_files::copy_verified(archive, &target)
            .await
            .map_err(|error| match error {
                VerifiedMoveError::TargetTaken(_) => DestinationError::NameTaken(name.to_owned()),
                other => DestinationError::Failed(other.to_string()),
            })?;
        Ok(StoredBackup {
            location: target.display().to_string(),
        })
    }

    async fn list(&self) -> Result<Vec<ListedArchive>, DestinationError> {
        let mut entries = tokio::fs::read_dir(&self.path)
            .await
            .map_err(|error| DestinationError::Failed(error.to_string()))?;
        let mut archives = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| DestinationError::Failed(error.to_string()))?
        {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(metadata) = entry.metadata().await else {
                continue;
            };
            if metadata.is_file() && is_archive_name(&name) {
                archives.push(ListedArchive {
                    name,
                    size: metadata.len(),
                });
            }
        }
        archives.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(archives)
    }

    async fn fetch(&self, name: &str, into: &Path) -> Result<u64, DestinationError> {
        check_archive_name(name)?;
        tokio::fs::copy(self.path.join(name), into)
            .await
            .map_err(|error| io_failure(&error, name))
    }

    async fn remove(&self, name: &str) -> Result<(), DestinationError> {
        check_archive_name(name)?;
        tokio::fs::remove_file(self.path.join(name))
            .await
            .map_err(|error| io_failure(&error, name))
    }
}

#[cfg(test)]
mod tests {
    use super::{BackupDestination, DestinationError, LocalFolder, is_archive_name};

    #[tokio::test]
    async fn an_archive_lands_under_its_name_and_is_never_overwritten() {
        let directory = tempfile::tempdir().expect("temp");
        let folder = LocalFolder::open(&directory.path().join("nas"))
            .await
            .expect("folder");
        let staged = directory.path().join("staged.rdbackup");
        std::fs::write(&staged, b"sealed").expect("stage");
        let stored = folder
            .store(&staged, "rdownloader-backup.rdbackup")
            .await
            .expect("store");
        assert!(stored.location.ends_with("rdownloader-backup.rdbackup"));
        assert_eq!(
            std::fs::read(directory.path().join("nas/rdownloader-backup.rdbackup")).expect("read"),
            b"sealed"
        );
        // A copy: the staged archive is still there for the next destination.
        assert!(staged.exists());

        std::fs::write(&staged, b"other").expect("stage again");
        let taken = folder
            .store(&staged, "rdownloader-backup.rdbackup")
            .await
            .expect_err("taken");
        assert_eq!(taken.code(), "backup.destination_name_taken");
        assert_eq!(
            std::fs::read(directory.path().join("nas/rdownloader-backup.rdbackup")).expect("read"),
            b"sealed"
        );
        let escaping = folder.store(&staged, "../outside.rdbackup").await;
        assert!(matches!(escaping, Err(DestinationError::InvalidName(_))));
    }

    #[tokio::test]
    async fn a_relative_folder_is_refused_like_a_storage_root() {
        let refused = LocalFolder::open(std::path::Path::new("relative/backups"))
            .await
            .expect_err("relative");
        assert_eq!(refused.code(), "storage_root.path_not_absolute");
    }

    #[test]
    fn only_plain_archive_names_are_archive_names() {
        for good in [
            "rdownloader-backup-0a1b2c3d-20260928T030000Z.rdbackup",
            "a.rdbackup",
        ] {
            assert!(is_archive_name(good), "{good}");
        }
        for bad in [
            "",
            ".rdbackup",
            ".hidden.rdbackup",
            "../a.rdbackup",
            "sub/a.rdbackup",
            "a.rdbackup.partial",
            "notes.txt",
            "ardbackup",
        ] {
            assert!(!is_archive_name(bad), "{bad}");
        }
    }
}
