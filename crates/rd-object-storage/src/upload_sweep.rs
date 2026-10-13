//! The sweep of unfinished multipart uploads: the stale ones at start, a profile's before it
//! is deleted. Split from `upload.rs`, which re-exports [`STALE_UPLOAD_AGE`], so neither file
//! outgrows the 500-line rule (RD-1280-08).

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use object_store::path::Path;
use rd_core::ObjectStorageProfile;
use rd_db::ObjectUpload as UploadRecord;

use crate::ObjectStorageService;

/// How long an unfinished upload is kept before the sweep aborts it.
///
/// A week: long enough for any stopped package to be resumed by somebody who was away, short
/// enough that an abandoned one does not sit in the bucket as billed, invisible parts.
pub const STALE_UPLOAD_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

impl ObjectStorageService {
    /// Aborts the unfinished uploads started more than `age` ago and forgets every record of
    /// that age, finished or not.
    ///
    /// Run at start: a package that was removed while its upload was half done leaves parts
    /// in the bucket that are billed and invisible until somebody aborts them.
    pub async fn sweep_stale_uploads(&self, age: Duration) -> Result<usize> {
        let before = Utc::now() - chrono::Duration::from_std(age)?;
        let stale = self.database().object_uploads(None, Some(before)).await?;
        self.abort_uploads(stale).await
    }

    /// Aborts every unfinished upload of one profile, before the profile is deleted and its
    /// credentials with it.
    pub async fn abort_profile_uploads(&self, profile: &ObjectStorageProfile) -> Result<usize> {
        let uploads = self
            .database()
            .object_uploads(Some(profile.id), None)
            .await?;
        self.abort_uploads(uploads).await
    }

    async fn abort_uploads(&self, uploads: Vec<UploadRecord>) -> Result<usize> {
        let mut aborted = 0;
        for record in uploads {
            if let Some(upload_id) = &record.upload_id
                && let Some(profile) = self
                    .database()
                    .object_storage_profile(record.profile_id)
                    .await?
                && let Ok(store) = self.open(&profile, &record.bucket).await?
                && let Ok(location) = Path::parse(&record.object_key)
            {
                match store.parts.abort_multipart(&location, upload_id).await {
                    Ok(()) | Err(object_store::Error::NotFound { .. }) => aborted += 1,
                    Err(error) => {
                        tracing::warn!(%error, "a stale multipart upload could not be aborted");
                        continue;
                    }
                }
            }
            self.database()
                .forget_object_uploads(Some(record.id), None)
                .await?;
        }
        Ok(aborted)
    }
}
