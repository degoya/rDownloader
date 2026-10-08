//! The completion listener: requests a package's pipeline once its last file settled.

use anyhow::Result;
use rd_core::{DownloadFile, DownloadKind, DownloadState, EventKind};

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

    /// Requests the pipeline for `package` once [`ready_for_postprocess`] says its files are
    /// there, unless post-processing already started or ended.
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
        if !ready_for_postprocess(package.kind, &siblings) {
            return Ok(());
        }
        self.request(package.id, ExtractionTrigger::Auto).await
    }
}

/// Whether a package's files are where its post-processing can start (RD-1190-13): at least one
/// completed, and every one completed, stood down as a mirror, or cancelled by somebody.
///
/// A file that waits, failed or is blocked is a part that is missing. Until 1.19 a failed or
/// blocked one counted as settled, so a package whose account ran out of traffic was unpacked
/// with half its volumes, failed for it, and read as finished; it now waits, and the file that
/// completes last starts the pipeline. A Usenet set is the exception and starts once every file
/// settled, failed ones included, as before: PAR2 rebuilds a missing file.
pub(crate) fn ready_for_postprocess(kind: DownloadKind, files: &[DownloadFile]) -> bool {
    let settled = |state: DownloadState| {
        done(state) || matches!(state, DownloadState::Failed | DownloadState::Blocked)
    };
    let any_completed = files.iter().any(|file| {
        matches!(
            file.state,
            DownloadState::Completed | DownloadState::Seeding
        )
    });
    any_completed
        && if kind == DownloadKind::Usenet {
            files.iter().all(|file| settled(file.state))
        } else {
            files.iter().all(|file| done(file.state))
        }
}

/// A file post-processing does not wait for: completed (a seeding torrent is), cancelled by
/// somebody, or a mirror that was never needed.
fn done(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Completed
            | DownloadState::Seeding
            | DownloadState::Cancelled
            | DownloadState::Skipped
    )
}

/// Whether a package lacks a file it was meant to hold, outside Usenet, where PAR2 may have
/// rebuilt it. Such a package is never `Completed` (RD-1190-13), whatever its steps made of the
/// rest.
pub(crate) fn parts_missing(kind: DownloadKind, files: &[DownloadFile]) -> bool {
    kind != DownloadKind::Usenet && !files.iter().all(|file| done(file.state))
}
