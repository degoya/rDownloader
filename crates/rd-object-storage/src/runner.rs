//! Object storage as a scheduler runner: one object per queue entry.

use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use object_store::{GetOptions, GetRange, path::Path};
use rd_core::{
    DownloadFile, DownloadKind, DownloadPackage, ObjectAddress, ObjectStorageProvider,
    StorageRootId,
};
use rd_files::StorageRoot;
use rd_scheduler::{ExternalRunner, RunLimits, RunOutcome};
use rd_transfer_file::{Labels, Resume, Staging};
use tokio_util::sync::CancellationToken;

use crate::{ObjectStorageService, error, head};

/// The stable code and the wording this crate puts on the shared transfer's failures.
const LABELS: Labels = Labels {
    length_mismatch: error::LENGTH_MISMATCH,
    length_mismatch_message: "The object download did not deliver the expected number of bytes",
    stalled: "the object storage service stopped sending data",
    open_part: "open the partial object download",
    flush_part: "flush the object download",
};

/// Downloads one object per queue row.
pub struct ObjectStorageRunner {
    service: ObjectStorageService,
}

impl ObjectStorageRunner {
    #[must_use]
    pub const fn new(service: ObjectStorageService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl ExternalRunner for ObjectStorageRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::ObjectStorage
    }

    /// The staged part file resumes once the object's validator (ETag, or Google's generation)
    /// still matches; a finished file is not adopted, and the ETag is no digest to verify.
    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: true,
            recheck_partial: true,
            adopt_completed: false,
            verify_completed: false,
            applies_collision_policy: false,
        }
    }

    fn slot_capacity(&self) -> usize {
        self.service.max_parallel()
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        limits: RunLimits,
    ) -> Result<RunOutcome> {
        let Some(address) = ObjectAddress::parse(&file.source).filter(|a| !a.is_prefix()) else {
            return Ok(RunOutcome::Failed(error::address_invalid()));
        };
        let Ok(location) = Path::parse(&address.key) else {
            return Ok(RunOutcome::Failed(error::address_invalid()));
        };
        if package.destination.is_empty() {
            anyhow::bail!("object storage package has no destination directory");
        }
        let root = StorageRoot::create(
            StorageRootId::new(),
            "download destination".to_owned(),
            PathBuf::from(&package.destination),
        )
        .await?;
        tokio::fs::create_dir_all(root.path()).await?;
        let part_path = rd_files::part_path(&root, file.id).await?;

        // Chosen again at every start rather than pinned at enqueue, like an FTP login: a
        // profile that was fixed in the meantime is the one the retry should use.
        let profile = match self.service.resolve(&address).await? {
            Ok(profile) => profile,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        let store = match self.service.open(&profile, &address.bucket).await? {
            Ok(store) => store,
            Err(failure) => return Ok(RunOutcome::Failed(failure)),
        };
        let meta = match head(&store, &location).await {
            Ok(meta) => meta,
            Err(error) => {
                return Ok(RunOutcome::Failed(error::classify(&error, &address.bucket)));
            }
        };

        let staging = Staging::open(
            self.service.database(),
            file,
            &root,
            &part_path,
            meta.size,
            LABELS,
        )
        .await;
        // The ETag is the object's version: compared as a string, never read as a digest.
        // Multipart and KMS-encrypted objects carry one that is not the content's MD5, and a
        // compatible service owes nothing either way.
        let resume = match staging
            .plan_resume_validated(validator(&meta), Some(meta.last_modified.to_rfc3339()))
            .await?
        {
            Resume::Refused => return Ok(RunOutcome::Failed(error::object_changed())),
            Resume::Complete => return staging.promote().await,
            other => other,
        };
        let offset = if matches!(resume, Resume::Continue) {
            staging.committed()
        } else {
            0
        };
        // `If-Match` makes the service refuse the ranged read when the object was replaced
        // between the `HEAD` above and this request, which the staging cannot see. Google
        // also reads exactly the generation the `HEAD` saw; S3 and Azure are not asked for
        // their version id, which needs a permission (`s3:GetObjectVersion`) a plain read
        // key does not have, and the ETag already changes with every write.
        let options = GetOptions {
            if_match: meta.e_tag.clone(),
            version: meta
                .version
                .clone()
                .filter(|_| profile.provider == ObjectStorageProvider::Gcs),
            range: (offset > 0).then_some(GetRange::Offset(offset)),
            ..GetOptions::default()
        };
        let body = match store.objects.get_opts(&location, options).await {
            Ok(body) => body,
            Err(error) => {
                return Ok(RunOutcome::Failed(error::classify(&error, &address.bucket)));
            }
        };
        // A service that ignored the range would stream from byte zero onto the end of the
        // partial file; the staging's length check would catch it, but only after the whole
        // object went through a second time.
        if body.range.start != offset {
            return Ok(RunOutcome::Failed(error::object_changed()));
        }
        let mut reader =
            tokio_util::io::StreamReader::new(body.into_stream().map_err(std::io::Error::from));
        let mut sink = staging.open_part().await?;
        let end = staging
            .stream(
                &mut reader,
                &mut sink,
                &limits.bandwidth,
                &cancellation,
                self.service.timeout(),
            )
            .await?;
        staging.finish(sink, end).await
    }
}

/// What the staging compares to decide whether the partial file still belongs to the object:
/// the ETag, and the provider's version when it reports one — Google's generation, which
/// every overwrite changes, or the version id of a versioned S3 bucket or Azure container.
/// Google's metageneration is left out on purpose: it counts metadata edits, which leave the
/// bytes alone.
pub(crate) fn validator(meta: &object_store::ObjectMeta) -> Option<String> {
    match (&meta.e_tag, &meta.version) {
        (Some(etag), Some(version)) => Some(format!("{etag}; version={version}")),
        (Some(etag), None) => Some(etag.clone()),
        (None, Some(version)) => Some(format!("version={version}")),
        (None, None) => None,
    }
}
