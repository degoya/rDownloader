//! The upload step's object storage half: multipart uploads that survive a restart.
//!
//! Every part the service confirms is recorded with the identifier the completion has to name
//! it by, so a run that was stopped — or a process that was killed — continues at the first
//! missing part rather than at byte zero. The record is keyed on the destination object and
//! remembers the local file's size and modification time: a file that changed in between
//! gets its old upload aborted and starts over, because its parts would describe a file that
//! no longer exists.
//!
//! Nothing local is deleted on the strength of the upload call alone. After the completion a
//! `HEAD` has to report the local size before the file counts as uploaded — the size and
//! nothing more, because what an `ETag` contains depends on the service and the upload path.
//!
//! Every part is paced by the upload limit (RD-150-15) before it is sent; see [`read_part`].

use std::{path::Path as LocalPath, time::Duration};

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use object_store::{PutOptions, PutPayload, multipart::PartId, path::Path};
use rd_core::{ObjectStorageProfile, ObjectStorageProfileId};
use rd_db::{ObjectUpload as UploadRecord, ObjectUploadPart};
use rd_extract::{ObjectUpload, ObjectUploader, UploadReport};
use rd_limits::ScopedLimiter;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;

use crate::{ObjectStorageService, connect::Store, head};

/// Smallest part used. S3 and Google refuse parts below 5 MiB except the last; 16 MiB keeps
/// the number of requests — and of rows recorded — low without holding much in memory.
const MIN_PART_SIZE: u64 = 16 * 1024 * 1024;
/// S3's and Google's ceiling on parts per upload; Azure's is 50 000 blocks.
const MAX_PARTS: u64 = 10_000;
/// The slice a part's bytes are acquired from the upload limit in: small enough that a profile
/// switch or a stop reaches a waiting part within a moment, large enough to cost nothing when
/// the upload is unlimited.
const PACE_SLICE: usize = 256 * 1024;
/// How long an unfinished upload is kept before the sweep aborts it.
///
/// A week: long enough for any stopped package to be resumed by somebody who was away, short
/// enough that an abandoned one does not sit in the bucket as billed, invisible parts.
pub const STALE_UPLOAD_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The part size for a file: at least [`MIN_PART_SIZE`], and large enough to stay within
/// [`MAX_PARTS`], rounded up to whole mebibytes.
pub(crate) fn part_size(size: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    let needed = size.div_ceil(MAX_PARTS).div_ceil(MIB) * MIB;
    needed.max(MIN_PART_SIZE)
}

/// Where one file goes: `<prefix>/<package>/<file>`, the layout the rclone path uses.
pub(crate) fn object_key(prefix: &str, package_name: &str, file: &str) -> String {
    let mut key = String::new();
    let prefix = prefix.trim_matches('/');
    if !prefix.is_empty() {
        key.push_str(prefix);
        key.push('/');
    }
    key.push_str(&rd_files::sanitize_file_name(package_name));
    key.push('/');
    key.push_str(&file.replace('\\', "/"));
    key
}

/// Splits `<bucket>/<prefix>`; an empty bucket falls back to the profile's bound one.
pub(crate) fn split_destination<'a>(
    provider: rd_core::ObjectStorageProvider,
    destination: &'a str,
    bound: Option<&'a str>,
) -> Option<(&'a str, &'a str)> {
    let destination = destination.trim_matches('/');
    let (bucket, prefix) = destination.split_once('/').unwrap_or((destination, ""));
    let bucket = if bucket.is_empty() { bound? } else { bucket };
    provider.is_valid_bucket(bucket).then_some((bucket, prefix))
}

#[async_trait]
impl ObjectUploader for ObjectStorageService {
    async fn upload(&self, profile_id: &str, upload: ObjectUpload<'_>) -> Result<UploadReport> {
        let Ok(id) = profile_id.parse::<ObjectStorageProfileId>() else {
            return Ok(failed("the upload target names no object storage profile"));
        };
        let Some(profile) = self.database().object_storage_profile(id).await? else {
            return Ok(failed(
                "the object storage profile of this upload target was deleted",
            ));
        };
        if !profile.enabled {
            return Ok(failed(
                "the object storage profile of this upload target is switched off",
            ));
        }
        let Some((bucket, prefix)) = split_destination(
            profile.provider,
            upload.destination,
            profile.bucket.as_deref(),
        ) else {
            return Ok(failed("the upload target names no valid bucket"));
        };
        let store = match self.open(&profile, bucket).await? {
            Ok(store) => store,
            Err(failure) => return Ok(failed(&failure.message)),
        };
        let mut sizes = Vec::with_capacity(upload.files.len());
        for file in upload.files {
            let metadata = tokio::fs::metadata(upload.directory.join(file))
                .await
                .with_context(|| "read an uploaded file's size")?;
            sizes.push(metadata.len());
        }
        let total: u64 = sizes.iter().sum();
        let mut done = 0_u64;
        let mut uploaded = Vec::with_capacity(upload.files.len());
        for (file, size) in upload.files.iter().zip(sizes) {
            let key = object_key(prefix, upload.package_name, file);
            let job = FileUpload {
                service: self,
                store: &store,
                profile: &profile,
                bucket,
                key: &key,
                owner: upload.owner,
                local: &upload.directory.join(file),
                size,
                bandwidth: &upload.bandwidth,
            };
            let base = done;
            let progress = |sent: u64| (upload.progress)(base + sent, Some(total));
            match job.run(&upload.stop, &progress).await? {
                FileOutcome::Uploaded => uploaded.push(file.clone()),
                FileOutcome::Stopped => return Ok(UploadReport::Stopped),
                FileOutcome::Failed(message) => return Ok(UploadReport::Failed { message }),
            }
            done += size;
        }
        // Every file is in and verified; nothing is left to continue.
        self.database()
            .forget_object_uploads(None, Some(upload.owner.to_owned()))
            .await?;
        Ok(UploadReport::Verified { files: uploaded })
    }
}

fn failed(message: &str) -> UploadReport {
    UploadReport::Failed {
        message: message.to_owned(),
    }
}

pub(crate) enum FileOutcome {
    Uploaded,
    Stopped,
    Failed(String),
}

pub(crate) struct FileUpload<'a> {
    pub service: &'a ObjectStorageService,
    pub store: &'a Store,
    pub profile: &'a ObjectStorageProfile,
    pub bucket: &'a str,
    pub key: &'a str,
    pub owner: &'a str,
    pub local: &'a LocalPath,
    pub size: u64,
    pub bandwidth: &'a ScopedLimiter,
}

impl FileUpload<'_> {
    pub(crate) async fn run(
        &self,
        stop: &CancellationToken,
        progress: &(dyn Fn(u64) + Sync),
    ) -> Result<FileOutcome> {
        let Ok(location) = Path::parse(self.key) else {
            return Ok(FileOutcome::Failed(
                "the file name cannot be used as an object key".to_owned(),
            ));
        };
        let modified = modified_at(self.local).await;
        let part_size = part_size(self.size);
        let database = self.service.database();
        let recorded = database
            .object_upload(self.profile.id, self.bucket, self.key)
            .await?;
        let matches = |record: &UploadRecord| {
            record.local_size == self.size
                && record.local_modified == modified
                && record.part_size == part_size
                && record.checksums == self.profile.checksums
        };
        let mut resumable = None;
        if let Some(record) = recorded {
            if record.completed_at.is_some() && matches(&record) {
                // Finished in an earlier run that did not get to the end of the package.
                return self.verify(&location).await;
            }
            if record.upload_id.is_some() && matches(&record) {
                resumable = Some(record);
            } else {
                self.abandon(&location, &record).await?;
            }
        }

        if self.size <= part_size && resumable.is_none() {
            return self
                .single_request(&location, modified, part_size, stop, progress)
                .await;
        }
        let record = match resumable {
            Some(record) => record,
            None => {
                let upload_id = match self.store.parts.create_multipart(&location).await {
                    Ok(id) => id,
                    Err(error) => return Ok(self.refused(&error)),
                };
                let record = self.record(upload_id.into(), modified, part_size);
                database.begin_object_upload(record.clone()).await?;
                record
            }
        };
        let Some(upload_id) = record.upload_id.clone() else {
            return Ok(FileOutcome::Failed(
                "the upload record has no id".to_owned(),
            ));
        };
        let count = self.size.div_ceil(part_size).max(1);
        let mut parts: Vec<Option<String>> = vec![None; usize::try_from(count)?];
        let mut sent = 0_u64;
        for part in &record.parts {
            if let Some(slot) = parts.get_mut(part.part_number as usize) {
                *slot = Some(part.content_id.clone());
                sent += part.size;
            }
        }
        progress(sent);
        let mut source = tokio::fs::File::open(self.local)
            .await
            .context("open the file to upload")?;
        for (index, slot) in parts.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            if stop.is_cancelled() {
                return Ok(FileOutcome::Stopped);
            }
            let offset = index as u64 * part_size;
            let length = part_size.min(self.size - offset);
            let Some(payload) =
                read_part(&mut source, offset, length, self.bandwidth, stop).await?
            else {
                return Ok(FileOutcome::Stopped);
            };
            let part = match self
                .store
                .parts
                .put_part(&location, &upload_id, index, payload)
                .await
            {
                Ok(part) => part,
                Err(error) => return Ok(self.refused(&error)),
            };
            rd_core::failpoint!("object_storage.after_part_upload", || anyhow::anyhow!(
                "crash point"
            ));
            database
                .record_object_upload_part(
                    record.id.clone(),
                    ObjectUploadPart {
                        part_number: u32::try_from(index)?,
                        content_id: part.content_id.clone(),
                        size: length,
                    },
                )
                .await?;
            *slot = Some(part.content_id);
            sent += length;
            progress(sent);
        }
        let ids = parts
            .into_iter()
            .map(|content_id| PartId {
                content_id: content_id.unwrap_or_default(),
            })
            .collect();
        if let Err(error) = self
            .store
            .parts
            .complete_multipart(&location, &upload_id, ids)
            .await
        {
            return Ok(self.refused(&error));
        }
        database.complete_object_upload(record.id.clone()).await?;
        self.verify(&location).await
    }

    async fn single_request(
        &self,
        location: &Path,
        modified: Option<String>,
        part_size: u64,
        stop: &CancellationToken,
        progress: &(dyn Fn(u64) + Sync),
    ) -> Result<FileOutcome> {
        let mut source = tokio::fs::File::open(self.local)
            .await
            .context("open the file to upload")?;
        let Some(payload) = read_part(&mut source, 0, self.size, self.bandwidth, stop).await?
        else {
            return Ok(FileOutcome::Stopped);
        };
        if let Err(error) = self
            .store
            .objects
            .put_opts(location, payload, PutOptions::default())
            .await
        {
            return Ok(self.refused(&error));
        }
        // Recorded as finished, so a later run of the same package skips it after a `HEAD`.
        let record = self.record(None, modified, part_size);
        let id = record.id.clone();
        let database = self.service.database();
        database.begin_object_upload(record).await?;
        database.complete_object_upload(id).await?;
        progress(self.size);
        self.verify(location).await
    }

    fn record(
        &self,
        upload_id: Option<String>,
        modified: Option<String>,
        part_size: u64,
    ) -> UploadRecord {
        UploadRecord {
            id: uuid::Uuid::now_v7().to_string(),
            profile_id: self.profile.id,
            owner: self.owner.to_owned(),
            bucket: self.bucket.to_owned(),
            object_key: self.key.to_owned(),
            local_path: self.local.to_string_lossy().into_owned(),
            local_size: self.size,
            local_modified: modified,
            part_size,
            upload_id,
            checksums: self.profile.checksums,
            completed_at: None,
            created_at: Utc::now(),
            parts: Vec::new(),
        }
    }

    /// Aborts an upload whose parts no longer describe the local file, and forgets it.
    async fn abandon(&self, location: &Path, record: &UploadRecord) -> Result<()> {
        if let Some(upload_id) = &record.upload_id
            && let Err(error) = self.store.parts.abort_multipart(location, upload_id).await
            && !matches!(error, object_store::Error::NotFound { .. })
        {
            // Left to the bucket's lifecycle rule or the next sweep; starting the new upload
            // matters more than cleaning up the old one.
            tracing::warn!(%error, "an outdated multipart upload could not be aborted");
        }
        self.service
            .database()
            .forget_object_uploads(Some(record.id.clone()), None)
            .await?;
        Ok(())
    }

    /// The commit before any delete: the service has to report the local size.
    async fn verify(&self, location: &Path) -> Result<FileOutcome> {
        match head(self.store, location).await {
            Ok(meta) if meta.size == self.size => Ok(FileOutcome::Uploaded),
            Ok(meta) => Ok(FileOutcome::Failed(format!(
                "the service holds {} bytes of a {}-byte file",
                meta.size, self.size
            ))),
            Err(error) => Ok(self.refused(&error)),
        }
    }

    fn refused(&self, error: &object_store::Error) -> FileOutcome {
        let failure = crate::error::classify(error, self.bucket);
        FileOutcome::Failed(format!(
            "{} ({})",
            failure.message,
            failure.code.unwrap_or_default()
        ))
    }
}

/// Reads one part, paced by the upload limit; `None` when the upload was stopped while it
/// waited.
///
/// `object_store` sends a payload as fast as the connection allows, so the pace is kept by
/// holding the part back until the limiter has released its bytes, slice by slice. The rate
/// therefore holds on average over the upload rather than within one part, a profile switch
/// takes effect at the next slice, and a stop does not wait out a whole part's quota.
async fn read_part(
    source: &mut tokio::fs::File,
    offset: u64,
    length: u64,
    bandwidth: &ScopedLimiter,
    stop: &CancellationToken,
) -> Result<Option<PutPayload>> {
    source.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut buffer = vec![0_u8; usize::try_from(length)?];
    for slice in buffer.chunks_mut(PACE_SLICE) {
        source
            .read_exact(slice)
            .await
            .context("read a part of the file to upload")?;
        // The quota first: a stop ends a wait, it does not cut short a part that needs none.
        tokio::select! {
            biased;
            acquired = bandwidth.acquire(slice.len()) => acquired?,
            () = stop.cancelled() => return Ok(None),
        }
    }
    Ok(Some(PutPayload::from(buffer)))
}

async fn modified_at(path: &LocalPath) -> Option<String> {
    let modified = tokio::fs::metadata(path).await.ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified).to_rfc3339())
}

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
