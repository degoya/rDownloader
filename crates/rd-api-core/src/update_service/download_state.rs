//! The offered update's artifact, downloaded and verified ahead of the install (RD-180-02, owner
//! 2026-10-01): "Download" in the interface fetches it in the background into
//! `<data>/update/download/`, held to the signed manifest's size and SHA-256 like the install's own
//! download, and "Install and restart" then uses that file instead of fetching it again.
//!
//! One download at a time, whoever asks: the background download and an install wait for each
//! other on one lock, and the second finds the first's verified file ([`rd_update::verified_file`],
//! read and hashed again) instead of writing the same `.partial` twice. What it is doing is held in
//! memory; after a restart a second click finds the kept file in a moment.

use std::path::PathBuf;

use rd_update::{Artifact, UpdateError, install};

use super::{Download, UpdateService};
use crate::dto::UpdateDownloadStatus;

impl UpdateService {
    /// The verified artifact of `version`: the one already on disk, or downloaded now. Records what
    /// it does for [`Self::download_status`].
    ///
    /// # Errors
    ///
    /// What the download or the verification refused, with its stable code.
    pub async fn fetch_artifact(
        &self,
        version: &str,
        artifact: &Artifact,
    ) -> Result<PathBuf, UpdateError> {
        let _one_at_a_time = self.0.fetching.lock().await;
        let directory = install::update_dir(&self.data_dir()).join(install::DOWNLOAD_DIR);
        self.record_download(version, artifact, "downloading", 0, None);
        if let Some(file) = rd_update::verified_file(artifact, &directory).await {
            self.record_download(version, artifact, "ready", artifact.size, None);
            return Ok(file);
        }
        // A fetcher that cannot be had fails the download like any other refusal: returned
        // early, it left the status at "downloading" for good and logged nothing (audit Q2).
        let received = |bytes: u64| self.record_received(version, bytes);
        let outcome = match self.fetcher() {
            Ok(fetcher) => {
                rd_update::download_verified_with(fetcher.as_ref(), artifact, &directory, &received)
                    .await
            }
            Err(error) => Err(error),
        };
        match &outcome {
            Ok(_) => self.record_download(version, artifact, "ready", artifact.size, None),
            Err(error) => {
                tracing::warn!(code = error.code(), %error, version, "the update was not downloaded");
                self.record_download(
                    version,
                    artifact,
                    "failed",
                    0,
                    Some(error.code().to_owned()),
                );
            }
        }
        outcome
    }

    /// Starts [`Self::fetch_artifact`] behind the caller and answers with where it stands; a
    /// download of `version` that is already running is joined, not started twice.
    pub fn start_download(&self, version: &str, artifact: &Artifact) -> UpdateDownloadStatus {
        let running = self
            .download_status(version)
            .is_some_and(|status| status.state == "downloading");
        if !running {
            self.record_download(version, artifact, "downloading", 0, None);
            let (service, version, artifact) = (self.clone(), version.to_owned(), artifact.clone());
            // Nothing is lost here: `fetch_artifact` logs a failure and records it for
            // `download_status`, which is where the interface reads it.
            tokio::spawn(async move {
                let _ = service.fetch_artifact(&version, &artifact).await;
            });
        }
        self.download_status(version)
            .unwrap_or_else(|| UpdateDownloadStatus {
                version: version.to_owned(),
                state: "downloading".to_owned(),
                received_bytes: 0,
                total_bytes: artifact.size,
                reason: None,
            })
    }

    /// The download of `version`, if one was asked for since the start.
    #[must_use]
    pub fn download_status(&self, version: &str) -> Option<UpdateDownloadStatus> {
        let download = self.0.download.lock().ok()?.clone()?;
        (download.version == version).then(|| UpdateDownloadStatus {
            version: download.version,
            state: download.state.to_owned(),
            received_bytes: download.received,
            total_bytes: download.total,
            reason: download.reason,
        })
    }

    fn record_download(
        &self,
        version: &str,
        artifact: &Artifact,
        state: &'static str,
        received: u64,
        reason: Option<String>,
    ) {
        if let Ok(mut download) = self.0.download.lock() {
            *download = Some(Download {
                version: version.to_owned(),
                state,
                received,
                total: artifact.size,
                reason,
            });
        }
    }

    fn record_received(&self, version: &str, bytes: u64) {
        if let Ok(mut download) = self.0.download.lock()
            && let Some(download) = download
                .as_mut()
                .filter(|download| download.version == version)
        {
            download.received = bytes;
        }
    }
}
