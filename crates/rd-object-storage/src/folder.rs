//! One folder of a bucket as a plain file store: one file up, a listing, one file down, one
//! delete (RD-160-02).
//!
//! The full backup's object storage destination is built on this. It takes the same profiles,
//! the same `<bucket>/<prefix>` destinations ([`split_destination`]) and the same upload as the
//! post-processing step: a file above one part goes up as a multipart upload that is recorded
//! part by part, so a retry continues at the first missing part, every part is paced by the
//! upload limit (RD-150-15), and a `HEAD` has to report the local size before the upload
//! counts. A multipart upload is invisible until it completes, which is what makes the commit
//! atomic: nothing appears under a name unless it is the whole file.
//!
//! Names are single path segments below the folder; a name with a slash is refused, so the
//! folder is all this can reach.

use std::path::Path as LocalPath;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use object_store::{GetOptions, ObjectStoreExt, path::Path};
use rd_core::{Failure, FailureKind, ObjectStorageProfile, ObjectStorageProfileId};
use rd_limits::ScopedLimiter;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::{
    ObjectStorageService,
    connect::Store,
    error, head,
    upload::{FileOutcome, FileUpload, split_destination},
};

/// Stable code of a profile id that names no profile, or a deleted one.
pub const FOLDER_PROFILE_MISSING: &str = "object_storage.profile_missing";
/// Stable code of a folder whose destination names no valid bucket.
pub const FOLDER_BUCKET_INVALID: &str = "object_storage.bucket_invalid";
/// Stable code of a name that is not one plain path segment.
pub const FOLDER_NAME_INVALID: &str = "object_storage.name_invalid";

/// One object directly in the folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderObject {
    /// The name below the folder.
    pub name: String,
    pub size: u64,
    pub modified: DateTime<Utc>,
}

/// A folder of a bucket, opened with its profile's credentials.
#[derive(Clone)]
pub struct ObjectFolder {
    service: ObjectStorageService,
    profile: ObjectStorageProfile,
    store: Store,
    bucket: String,
    prefix: String,
}

impl std::fmt::Debug for ObjectFolder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObjectFolder")
            .field("profile", &self.profile.id)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

fn permanent(code: &str, message: &str) -> Failure {
    Failure::coded(FailureKind::Permanent, code, message)
}

impl ObjectStorageService {
    /// Opens `<bucket>/<prefix>` of a profile; an empty bucket is the profile's bound one.
    ///
    /// Only the profile and the credentials are checked here; whether the bucket answers is
    /// the first request's business.
    pub async fn open_folder(
        &self,
        profile_id: &str,
        destination: &str,
    ) -> Result<Result<ObjectFolder, Failure>> {
        let Ok(id) = profile_id.parse::<ObjectStorageProfileId>() else {
            return Ok(Err(permanent(
                FOLDER_PROFILE_MISSING,
                "The destination names no object storage profile",
            )));
        };
        let Some(profile) = self.database().object_storage_profile(id).await? else {
            return Ok(Err(permanent(
                FOLDER_PROFILE_MISSING,
                "The object storage profile of this destination was deleted",
            )));
        };
        if !profile.enabled {
            return Ok(Err(permanent(
                error::PROFILE_DISABLED,
                "The object storage profile of this destination is switched off",
            )));
        }
        let Some((bucket, prefix)) =
            split_destination(profile.provider, destination, profile.bucket.as_deref())
        else {
            return Ok(Err(permanent(
                FOLDER_BUCKET_INVALID,
                "The destination names no valid bucket",
            )));
        };
        let (bucket, prefix) = (bucket.to_owned(), prefix.trim_matches('/').to_owned());
        let store = match self.open(&profile, &bucket).await? {
            Ok(store) => store,
            Err(failure) => return Ok(Err(failure)),
        };
        Ok(Ok(ObjectFolder {
            service: self.clone(),
            profile,
            store,
            bucket,
            prefix,
        }))
    }
}

impl ObjectFolder {
    /// `<bucket>/<prefix>` as the history names it.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.prefix.is_empty() {
            self.bucket.clone()
        } else {
            format!("{}/{}", self.bucket, self.prefix)
        }
    }

    fn key(&self, name: &str) -> Result<(String, Path), Failure> {
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err(permanent(
                FOLDER_NAME_INVALID,
                "The name is not a plain file name",
            ));
        }
        let key = if self.prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.prefix)
        };
        let location = Path::parse(&key)
            .map_err(|_| permanent(FOLDER_NAME_INVALID, "The name cannot be an object key"))?;
        Ok((key, location))
    }

    /// Whether an object is there under `name`, and its size.
    pub async fn size_of(&self, name: &str) -> Result<Option<u64>, Failure> {
        let (_, location) = self.key(name)?;
        match head(&self.store, &location).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(other) => Err(error::classify(&other, &self.bucket)),
        }
    }

    /// Uploads `local` under `name` and checks the service holds all of it. `owner` keys the
    /// part records, so a retry with the same owner continues where the last attempt stopped.
    ///
    /// # Errors
    ///
    /// The outer error is the database's; the inner one the service's, with its stable code.
    pub async fn put_file(
        &self,
        name: &str,
        local: &LocalPath,
        owner: &str,
        bandwidth: &ScopedLimiter,
        stop: &CancellationToken,
    ) -> Result<Result<u64, Failure>> {
        let (key, _) = match self.key(name) {
            Ok(key) => key,
            Err(failure) => return Ok(Err(failure)),
        };
        let size = tokio::fs::metadata(local)
            .await
            .context("read the size of the file to upload")?
            .len();
        let job = FileUpload {
            service: &self.service,
            store: &self.store,
            profile: &self.profile,
            bucket: &self.bucket,
            key: &key,
            owner,
            local,
            size,
            bandwidth,
        };
        let outcome = job.run(stop, &|_: u64| {}).await?;
        match outcome {
            FileOutcome::Uploaded => {
                self.service
                    .database()
                    .forget_object_uploads(None, Some(owner.to_owned()))
                    .await?;
                Ok(Ok(size))
            }
            FileOutcome::Stopped => Ok(Err(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                error::UPLOAD_FAILED,
                "The upload was stopped",
            ))),
            FileOutcome::Failed(message) => Ok(Err(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                error::UPLOAD_FAILED,
                message,
            ))),
        }
    }

    /// Every object directly in the folder, whoever wrote it. Deeper keys are not listed.
    pub async fn list(&self) -> Result<Vec<FolderObject>, Failure> {
        let scope = if self.prefix.is_empty() {
            None
        } else {
            Some(Path::parse(&self.prefix).map_err(|_| error::address_invalid())?)
        };
        let listing = self
            .store
            .objects
            .list_with_delimiter(scope.as_ref())
            .await
            .map_err(|error| error::classify(&error, &self.bucket))?;
        Ok(listing
            .objects
            .into_iter()
            .filter_map(|meta| {
                let name = meta.location.filename()?.to_owned();
                Some(FolderObject {
                    name,
                    size: meta.size,
                    modified: meta.last_modified,
                })
            })
            .collect())
    }

    /// Streams `name` into `local`, which must not exist yet; returns the bytes written. A
    /// file left by a failed download is removed.
    ///
    /// # Errors
    ///
    /// The outer error is the local disk's; the inner one the service's.
    pub async fn get_file(&self, name: &str, local: &LocalPath) -> Result<Result<u64, Failure>> {
        let (_, location) = match self.key(name) {
            Ok(key) => key,
            Err(failure) => return Ok(Err(failure)),
        };
        let result = match self
            .store
            .objects
            .get_opts(&location, GetOptions::default())
            .await
        {
            Ok(result) => result,
            Err(error) => return Ok(Err(error::classify(&error, &self.bucket))),
        };
        let mut file = tokio::fs::File::create_new(local)
            .await
            .with_context(|| format!("create {}", local.display()))?;
        let mut stream = result.into_stream();
        let mut written = 0_u64;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(local).await;
                    return Ok(Err(error::classify(&error, &self.bucket)));
                }
            };
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
        }
        file.sync_all().await?;
        Ok(Ok(written))
    }

    /// Deletes `name`; `Ok(false)` when nothing was there.
    pub async fn delete(&self, name: &str) -> Result<bool, Failure> {
        let (_, location) = self.key(name)?;
        // S3 answers a delete of a missing key with success, so the `HEAD` is what can say so.
        match head(&self.store, &location).await {
            Ok(_) => {}
            Err(object_store::Error::NotFound { .. }) => return Ok(false),
            Err(other) => return Err(error::classify(&other, &self.bucket)),
        }
        match self.store.objects.delete(&location).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(true),
            Err(other) => Err(error::classify(&other, &self.bucket)),
        }
    }
}
