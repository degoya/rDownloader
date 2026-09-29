//! The destinations beyond this machine (RD-160-02): a folder of an object storage bucket and
//! an rclone remote, and how a stored destination row becomes one of the three.
//!
//! Both reuse what the post-processing upload already has rather than a second copy of it:
//! object storage the profiles, the resumable multipart upload and the upload limit of
//! `rd_object_storage` ([`rd_object_storage::ObjectFolder`]); rclone the tool lookup, `--bwlimit`
//! and the argument order of `rd_extract` ([`rd_extract::RcloneRemote`]). There is no native
//! WebDAV destination (owner's decision, 2026-09-28): a WebDAV share is an rclone remote.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rd_core::{Failure, FailureKind};
use rd_extract::{RcloneFailure, RcloneRemote};
use rd_limits::ScopedLimiter;
use rd_object_storage::{ObjectFolder, ObjectStorageService};
use tokio_util::sync::CancellationToken;

use crate::destination::{
    BackupDestination, DestinationError, ListedArchive, LocalFolder, StoredBackup,
    check_archive_name, is_archive_name,
};

/// What opening a destination needs from the service.
#[derive(Clone)]
pub struct DestinationContext {
    pub object_storage: ObjectStorageService,
    /// The configured rclone binary; `None` looks in the vendor folder and on `PATH`.
    pub rclone_executable: Option<String>,
    pub vendor_directory: Option<String>,
    /// The upload limit every upload keeps (RD-150-15).
    pub bandwidth: ScopedLimiter,
}

/// A destination as its row describes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DestinationConfig {
    /// A folder on this machine or a mounted NAS share: `{"path": "..."}`.
    Local { path: PathBuf },
    /// A folder of a bucket: `{"profile_id": "...", "prefix": "<bucket>/<folder>"}`; an empty
    /// bucket is the profile's bound one.
    ObjectStorage { profile_id: String, prefix: String },
    /// An rclone remote folder: `{"remote": "name:path"}`.
    Rclone { remote: String },
}

/// The kind an object storage destination is stored as.
pub const OBJECT_STORAGE_KIND: &str = "object_storage";
/// The kind an rclone destination is stored as.
pub const RCLONE_KIND: &str = "rclone";
/// Stable code of an rclone remote that is not `name:path`.
pub const RCLONE_REMOTE_INVALID: &str = "backup.rclone_remote_invalid";
/// Stable code of a destination kind this build does not know.
pub const DESTINATION_KIND_UNKNOWN: &str = "backup.destination_kind_unknown";

fn text(config: &serde_json::Value, field: &str) -> Option<String> {
    config
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(|value| value.trim().to_owned())
}

/// Whether `remote` reads as an rclone `name:path`: a remote name, a colon, no flag and no
/// control character.
#[must_use]
pub fn is_rclone_remote(remote: &str) -> bool {
    let remote = remote.trim();
    let Some((name, _)) = remote.split_once(':') else {
        return false;
    };
    !name.is_empty()
        && !remote.starts_with('-')
        && remote.len() <= 1_024
        && !remote.chars().any(char::is_control)
        // A drive letter is a Windows path, not a remote.
        && !(name.len() == 1 && name.chars().all(|letter| letter.is_ascii_alphabetic()))
}

impl DestinationConfig {
    /// Reads a stored row; `None` when the kind is unknown or a field is missing.
    #[must_use]
    pub fn parse(kind: &str, config: &serde_json::Value) -> Option<Self> {
        match kind {
            LocalFolder::KIND => LocalFolder::path_of(config).map(|path| Self::Local { path }),
            OBJECT_STORAGE_KIND => Some(Self::ObjectStorage {
                profile_id: text(config, "profile_id").filter(|id| !id.is_empty())?,
                prefix: text(config, "prefix").unwrap_or_default(),
            }),
            RCLONE_KIND => text(config, "remote")
                .filter(|remote| !remote.is_empty())
                .map(|remote| Self::Rclone { remote }),
            _ => None,
        }
    }

    /// The kind stored in `backup_destinations.kind`.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Local { .. } => LocalFolder::KIND,
            Self::ObjectStorage { .. } => OBJECT_STORAGE_KIND,
            Self::Rclone { .. } => RCLONE_KIND,
        }
    }

    /// The `config_json` of the row.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Local { path } => LocalFolder::config_of(path),
            Self::ObjectStorage { profile_id, prefix } => {
                serde_json::json!({ "profile_id": profile_id, "prefix": prefix })
            }
            Self::Rclone { remote } => serde_json::json!({ "remote": remote }),
        }
    }

    /// How the history names the destination before it is opened.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Local { path } => path.display().to_string(),
            Self::ObjectStorage { prefix, .. } => format!("object storage {prefix}"),
            Self::Rclone { remote } => remote.clone(),
        }
    }

    /// Checks what can be checked without the network: the folder like a storage root, the
    /// profile and its bucket, the shape of the remote.
    ///
    /// # Errors
    ///
    /// The reason with its stable code.
    pub async fn validate(&self, context: &DestinationContext) -> Result<(), DestinationError> {
        match self {
            Self::Rclone { remote } if !is_rclone_remote(remote) => {
                Err(DestinationError::Misconfigured {
                    code: RCLONE_REMOTE_INVALID,
                    detail: "the rclone remote is not name:path".to_owned(),
                })
            }
            Self::Rclone { .. } => Ok(()),
            other => other.open(context).await.map(drop),
        }
    }

    /// Opens the destination for a run.
    ///
    /// # Errors
    ///
    /// When it cannot serve: an unusable folder, a deleted profile, no rclone.
    pub async fn open(
        &self,
        context: &DestinationContext,
    ) -> Result<Box<dyn BackupDestination>, DestinationError> {
        match self {
            Self::Local { path } => Ok(Box::new(LocalFolder::open(path).await?)),
            Self::ObjectStorage { profile_id, prefix } => {
                let folder = context
                    .object_storage
                    .open_folder(profile_id, prefix)
                    .await
                    .map_err(|error| DestinationError::Failed(format!("{error:#}")))?
                    .map_err(|failure| from_failure(&failure))?;
                Ok(Box::new(ObjectStorageDestination {
                    folder,
                    bandwidth: context.bandwidth.clone(),
                }))
            }
            Self::Rclone { remote } => {
                if !is_rclone_remote(remote) {
                    return Err(DestinationError::Misconfigured {
                        code: RCLONE_REMOTE_INVALID,
                        detail: "the rclone remote is not name:path".to_owned(),
                    });
                }
                let rate = context
                    .bandwidth
                    .binding_limit()
                    .map(|limit| limit.bytes_per_second);
                let remote = RcloneRemote::locate(
                    context.rclone_executable.as_deref(),
                    context.vendor_directory.as_deref(),
                    remote,
                    rate,
                )
                .map_err(|failure| from_rclone(failure, ""))?;
                Ok(Box::new(RcloneDestination { remote }))
            }
        }
    }
}

/// The object storage codes a destination passes on as they are; anything else is the
/// backup's own `backup.destination_unreachable`.
fn known_code(code: Option<&str>) -> &'static str {
    use rd_object_storage::error::{
        ACCESS_DENIED, AUTH_FAILED, CONNECT_FAILED, NO_PROFILE, NOT_FOUND, PROFILE_DISABLED,
        RATE_LIMITED, REQUEST_FAILED, UPLOAD_FAILED,
    };
    [
        ACCESS_DENIED,
        AUTH_FAILED,
        CONNECT_FAILED,
        NO_PROFILE,
        NOT_FOUND,
        PROFILE_DISABLED,
        RATE_LIMITED,
        REQUEST_FAILED,
        UPLOAD_FAILED,
        rd_object_storage::FOLDER_BUCKET_INVALID,
        rd_object_storage::FOLDER_NAME_INVALID,
        rd_object_storage::FOLDER_PROFILE_MISSING,
        rd_object_storage::ENDPOINT_INVALID,
        rd_object_storage::PROVIDER_UNSUPPORTED,
    ]
    .into_iter()
    .find(|known| Some(*known) == code)
    .unwrap_or("backup.destination_unreachable")
}

/// A refusal that another attempt cannot change is a configuration fault; the rest is an
/// outage and is tried again.
fn from_failure(failure: &Failure) -> DestinationError {
    let code = known_code(failure.code.as_deref());
    let detail = failure.message.clone();
    match failure.category {
        FailureKind::Permanent
        | FailureKind::AuthRequired
        | FailureKind::AccountInvalid
        | FailureKind::Unsupported => DestinationError::Misconfigured { code, detail },
        _ => DestinationError::Unavailable { code, detail },
    }
}

fn from_rclone(failure: RcloneFailure, name: &str) -> DestinationError {
    match failure {
        RcloneFailure::Missing => DestinationError::Misconfigured {
            code: "backup.rclone_missing",
            detail: failure.to_string(),
        },
        RcloneFailure::NotFound => DestinationError::NotFound(name.to_owned()),
        RcloneFailure::Spawn(_) => DestinationError::Failed(failure.to_string()),
        RcloneFailure::Failed { .. } | RcloneFailure::Unreadable(_) => {
            DestinationError::Unavailable {
                code: "backup.rclone_failed",
                detail: failure.to_string(),
            }
        }
    }
}

/// A folder of a bucket.
pub struct ObjectStorageDestination {
    folder: ObjectFolder,
    bandwidth: ScopedLimiter,
}

#[async_trait]
impl BackupDestination for ObjectStorageDestination {
    fn kind(&self) -> &'static str {
        OBJECT_STORAGE_KIND
    }

    fn describe(&self) -> String {
        self.folder.describe()
    }

    async fn store(&self, archive: &Path, name: &str) -> Result<StoredBackup, DestinationError> {
        check_archive_name(name)?;
        if self
            .folder
            .size_of(name)
            .await
            .map_err(|failure| from_failure(&failure))?
            .is_some()
        {
            return Err(DestinationError::NameTaken(name.to_owned()));
        }
        // The owner keys the part records: a retry of the same archive continues at the first
        // part the service did not confirm.
        self.folder
            .put_file(
                name,
                archive,
                &format!("backup:{name}"),
                &self.bandwidth,
                &CancellationToken::new(),
            )
            .await
            .map_err(|error| DestinationError::Failed(format!("{error:#}")))?
            .map_err(|failure| from_failure(&failure))?;
        Ok(StoredBackup {
            location: format!("{}/{name}", self.folder.describe()),
        })
    }

    async fn list(&self) -> Result<Vec<ListedArchive>, DestinationError> {
        let objects = self
            .folder
            .list()
            .await
            .map_err(|failure| from_failure(&failure))?;
        Ok(objects
            .into_iter()
            .filter(|object| is_archive_name(&object.name))
            .map(|object| ListedArchive {
                name: object.name,
                size: object.size,
            })
            .collect())
    }

    async fn fetch(&self, name: &str, into: &Path) -> Result<u64, DestinationError> {
        check_archive_name(name)?;
        self.folder
            .get_file(name, into)
            .await
            .map_err(|error| DestinationError::Failed(format!("{error:#}")))?
            .map_err(|failure| {
                if failure.code.as_deref() == Some(rd_object_storage::error::NOT_FOUND) {
                    DestinationError::NotFound(name.to_owned())
                } else {
                    from_failure(&failure)
                }
            })
    }

    async fn remove(&self, name: &str) -> Result<(), DestinationError> {
        check_archive_name(name)?;
        match self.folder.delete(name).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(DestinationError::NotFound(name.to_owned())),
            Err(failure) => Err(from_failure(&failure)),
        }
    }
}

/// An rclone remote folder.
pub struct RcloneDestination {
    remote: RcloneRemote,
}

impl RcloneDestination {
    /// A destination on an rclone remote already located, for tests with a stand-in binary.
    #[must_use]
    pub const fn new(remote: RcloneRemote) -> Self {
        Self { remote }
    }
}

#[async_trait]
impl BackupDestination for RcloneDestination {
    fn kind(&self) -> &'static str {
        RCLONE_KIND
    }

    fn describe(&self) -> String {
        self.remote.remote().to_owned()
    }

    async fn store(&self, archive: &Path, name: &str) -> Result<StoredBackup, DestinationError> {
        check_archive_name(name)?;
        if self.list().await?.iter().any(|listed| listed.name == name) {
            return Err(DestinationError::NameTaken(name.to_owned()));
        }
        // Up under a temporary name that is no archive name, then renamed: a remote that
        // writes in place (a local or SFTP backend) never shows a half archive under the
        // final name, and `list` never shows the temporary one.
        let temporary = format!("{name}.partial");
        if let Err(failure) = self.remote.upload(archive, &temporary).await {
            let _ = self.remote.delete(&temporary).await;
            return Err(from_rclone(failure, name));
        }
        self.remote
            .rename(&temporary, name)
            .await
            .map_err(|failure| from_rclone(failure, name))?;
        // The commit's check: the remote has to report the local size.
        let expected = tokio::fs::metadata(archive)
            .await
            .map_err(|error| DestinationError::Failed(error.to_string()))?
            .len();
        let listed = self.list().await?;
        match listed.iter().find(|listed| listed.name == name) {
            Some(found) if found.size == expected => Ok(StoredBackup {
                location: self.remote.path_of(name),
            }),
            Some(found) => Err(DestinationError::Failed(format!(
                "the remote holds {} bytes of a {expected}-byte archive",
                found.size
            ))),
            None => Err(DestinationError::Failed(
                "the archive is not at the remote after the upload".to_owned(),
            )),
        }
    }

    async fn list(&self) -> Result<Vec<ListedArchive>, DestinationError> {
        let entries = self
            .remote
            .list()
            .await
            .map_err(|failure| from_rclone(failure, ""))?;
        Ok(entries
            .into_iter()
            .filter(|entry| is_archive_name(&entry.name))
            .map(|entry| ListedArchive {
                name: entry.name,
                size: entry.size,
            })
            .collect())
    }

    async fn fetch(&self, name: &str, into: &Path) -> Result<u64, DestinationError> {
        check_archive_name(name)?;
        self.remote
            .download(name, into)
            .await
            .map_err(|failure| from_rclone(failure, name))?;
        tokio::fs::metadata(into)
            .await
            .map(|metadata| metadata.len())
            .map_err(|error| DestinationError::Failed(error.to_string()))
    }

    async fn remove(&self, name: &str) -> Result<(), DestinationError> {
        check_archive_name(name)?;
        self.remote
            .delete(name)
            .await
            .map_err(|failure| from_rclone(failure, name))
    }
}

#[cfg(test)]
mod tests {
    use super::{DestinationConfig, is_rclone_remote};

    #[test]
    fn a_row_reads_back_as_the_destination_it_was_saved_as() {
        for config in [
            DestinationConfig::Local {
                path: "/mnt/nas/backups".into(),
            },
            DestinationConfig::ObjectStorage {
                profile_id: "7".to_owned(),
                prefix: "bucket/rdownloader".to_owned(),
            },
            DestinationConfig::Rclone {
                remote: "webdav:backups".to_owned(),
            },
        ] {
            assert_eq!(
                DestinationConfig::parse(config.kind(), &config.to_json()),
                Some(config.clone())
            );
        }
        assert_eq!(
            DestinationConfig::parse("webdav", &serde_json::json!({})),
            None
        );
        assert_eq!(
            DestinationConfig::parse("object_storage", &serde_json::json!({ "prefix": "b" })),
            None
        );
    }

    #[test]
    fn an_rclone_remote_is_name_colon_path() {
        for good in ["webdav:backups", "gdrive:", "nas:rd/backups"] {
            assert!(is_rclone_remote(good), "{good}");
        }
        for bad in ["", "backups", "-x:y", ":path", "C:\\backups", "a:\nb"] {
            assert!(!is_rclone_remote(bad), "{bad}");
        }
    }
}
