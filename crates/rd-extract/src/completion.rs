//! The completion listener: requests a package's pipeline once its last file settled.

use anyhow::Result;
use rd_core::{DownloadState, EventKind};

use crate::{ExtractionService, ExtractionTrigger};

impl ExtractionService {
    pub(crate) async fn listen_for_completions(self) {
        let mut events = self.inner.database.subscribe();
        loop {
            let event = tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                event = events.recv() => match event {
                    Ok(event) => event,
                    // The missed events may have been the last completion of a package, and
                    // nothing else would ever ask for its post-processing: `recover()` only
                    // runs at start-up. So the packages are looked at once instead
                    // (audit 1.9.1, INTAKE-04).
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "event bus lagged; checking every unsettled package");
                        if let Err(error) = self.sweep_settled_packages().await {
                            tracing::warn!(%error, "post-processing sweep after a lag failed");
                        }
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            if event.kind != EventKind::DownloadState
                || !matches!(
                    event.payload.get("state").and_then(|value| value.as_str()),
                    // Seeding torrents have a complete payload, so postprocessing starts
                    // while they keep uploading. A mirror standing down is the last thing
                    // that can settle a package, so it has to wake the check too — without
                    // it the pipeline waits for an event that will never come.
                    Some("completed" | "seeding" | "failed" | "blocked" | "cancelled" | "skipped")
                )
            {
                continue;
            }
            let Some(download_id) = event
                .payload
                .get("download_id")
                .and_then(|value| value.as_str())
                .and_then(|value| value.parse::<rd_core::DownloadId>().ok())
            else {
                continue;
            };
            // Before the settle check: the file that settles a package is never one a direct
            // unpack still has to start with (RD-1100-07).
            if event.payload.get("state").and_then(|value| value.as_str()) == Some("completed") {
                crate::direct_unpack::on_volume_completed(&self.inner, download_id).await;
            }
            if let Err(error) = self.on_download_finished(download_id).await {
                tracing::debug!(%error, "post-processing trigger check failed");
            }
        }
    }

    /// Requests the pipeline once every file of the package reached a terminal state
    /// and at least one file completed.
    async fn on_download_finished(&self, download_id: rd_core::DownloadId) -> Result<()> {
        let Some(download) = self.inner.database.get_download(download_id).await? else {
            return Ok(());
        };
        let Some(package) = self.inner.database.get_package(download.package_id).await? else {
            return Ok(());
        };
        self.request_if_settled(&package).await
    }

    /// Runs [`Self::request_if_settled`] for every package that has not reached
    /// post-processing yet — what a lagged event bus may have hidden.
    ///
    /// One package that cannot be looked at does not end the sweep: the ones after it may be
    /// exactly what the lag hid (audit 1.9.1, RA-IN-05).
    pub(crate) async fn sweep_settled_packages(&self) -> Result<()> {
        for package in self.inner.database.list_packages().await? {
            if let Err(error) = self.request_if_settled(&package).await {
                tracing::warn!(package_id = %package.id, %error, "post-processing sweep skipped a package");
            }
        }
        Ok(())
    }

    /// Requests the pipeline for `package` once every file reached a terminal state and at
    /// least one completed, unless post-processing already started or ended.
    async fn request_if_settled(&self, package: &rd_core::DownloadPackage) -> Result<()> {
        if matches!(
            package.state,
            rd_core::PackageState::Postprocessing
                | rd_core::PackageState::Completed
                | rd_core::PackageState::Failed
        ) {
            return Ok(());
        }
        let siblings = self
            .inner
            .database
            .downloads_for_package(package.id)
            .await?;
        let all_terminal = siblings.iter().all(|item| {
            matches!(
                item.state,
                DownloadState::Completed
                    | DownloadState::Seeding
                    | DownloadState::Failed
                    | DownloadState::Blocked
                    | DownloadState::Cancelled
                    // A mirror that was never needed is as settled as one that failed.
                    | DownloadState::Skipped
            )
        });
        let any_completed = siblings.iter().any(|item| {
            matches!(
                item.state,
                DownloadState::Completed | DownloadState::Seeding
            )
        });
        if !all_terminal || !any_completed {
            return Ok(());
        }
        self.request(package.id, ExtractionTrigger::Auto).await
    }
}
