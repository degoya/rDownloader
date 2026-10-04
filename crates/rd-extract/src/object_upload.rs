//! Uploading a finished package to object storage (RD-150-04).
//!
//! The third way out of the pipeline, beside [`crate::rclone_job`] and
//! [`crate::storage_upload`], picked by an `object-storage:<profile>/<bucket>/<prefix>` remote.
//! It keeps the plugin path's rule — **commit before delete**: a local file goes only after
//! the service confirmed, separately from the upload, that it holds all of it. What it adds is
//! that an interrupted upload is continued part by part after a restart instead of started
//! over, which is what `crates/rd-object-storage` records the parts for.

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::storage_upload::{UploadProgress, UploadReport};

/// One package on its way to one bucket.
pub struct ObjectUpload<'a> {
    /// The post-processing owner (the package id); the upload records are kept under it.
    pub owner: &'a str,
    pub package_name: &'a str,
    pub directory: &'a Path,
    /// File paths relative to the package directory, `/` between folders (`Film/film.mkv`);
    /// the relative path is kept in the object key.
    pub files: &'a [String],
    /// `<bucket>/<prefix>` as configured; the bucket may be left out for a profile bound to one.
    pub destination: &'a str,
    pub progress: UploadProgress,
    /// Stopping between parts leaves the upload where it is for the next run.
    pub stop: CancellationToken,
    /// The upload limit the parts are paced by (RD-150-15).
    pub bandwidth: rd_limits::ScopedLimiter,
}

/// The configured object storage profiles, as this crate needs to see them.
#[async_trait]
pub trait ObjectUploader: Send + Sync {
    /// Uploads one package into the bucket of `profile_id` and verifies what arrived.
    async fn upload(&self, profile_id: &str, upload: ObjectUpload<'_>) -> Result<UploadReport>;
}

/// The profile and destination named by an `object-storage:<profile>/<destination>` remote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ObjectRemote<'a> {
    pub profile_id: &'a str,
    pub destination: &'a str,
}

/// Reads a configured remote as an object storage target, or `None` for any other kind.
pub(crate) fn parse_object_remote(remote: &str) -> Option<ObjectRemote<'_>> {
    let rest = remote.strip_prefix("object-storage:")?;
    let (profile_id, destination) = rest.split_once('/').unwrap_or((rest, ""));
    let profile_id = profile_id.trim();
    (!profile_id.is_empty()).then_some(ObjectRemote {
        profile_id,
        destination: destination.trim(),
    })
}

/// Runs the upload step into object storage and says how it ended, as the other two upload
/// paths do; `configured` is the remote the step was planned under.
// The same inputs the plugin upload path takes plus the package name the object keys start
// with; a struct would exist only for this one call.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    inner: &crate::Inner,
    owner: &str,
    configured: &str,
    remote: ObjectRemote<'_>,
    package_name: &str,
    directory: &Path,
    files: &[String],
    mode: crate::rclone_job::UploadMode,
) -> Result<crate::steps::StepEnd> {
    let upload = inner.objects.as_ref().map(|uploader| {
        move |progress: UploadProgress| async move {
            uploader
                .upload(
                    remote.profile_id,
                    ObjectUpload {
                        owner,
                        package_name,
                        directory,
                        files,
                        destination: remote.destination,
                        progress,
                        stop: inner.shutdown.child_token(),
                        bandwidth: inner.upload_limit(),
                    },
                )
                .await
        }
    });
    crate::upload_step::run(
        inner,
        crate::upload_step::UploadStep {
            owner,
            label: configured,
            // The bucket path, not the profile id in front of it; a bucket-bound profile has
            // none, and the package's own folder is what it uploads into.
            shown: if remote.destination.is_empty() {
                package_name
            } else {
                remote.destination
            },
            directory,
            mode,
            unavailable: "object storage uploads are not available",
        },
        upload,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{ObjectRemote, parse_object_remote};

    #[test]
    fn an_object_remote_names_its_profile_first() {
        assert_eq!(
            parse_object_remote("object-storage:0199aa00-0000-7000-8000-000000000001/media/in"),
            Some(ObjectRemote {
                profile_id: "0199aa00-0000-7000-8000-000000000001",
                destination: "media/in",
            })
        );
        // A profile bound to a bucket needs nothing after the slash.
        assert_eq!(
            parse_object_remote("object-storage:0199aa00-0000-7000-8000-000000000001/"),
            Some(ObjectRemote {
                profile_id: "0199aa00-0000-7000-8000-000000000001",
                destination: "",
            })
        );
    }

    #[test]
    fn other_remotes_are_left_to_their_own_path() {
        assert_eq!(parse_object_remote("gdrive:downloads"), None);
        assert_eq!(parse_object_remote("plugin:abc/https://dav.example"), None);
        assert_eq!(parse_object_remote("object-storage:/bucket"), None);
    }
}
