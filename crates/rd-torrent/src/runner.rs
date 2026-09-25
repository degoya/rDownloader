//! Queue runner: one row drives one torrent — add to the session, download into the
//! package folder, then either hand over to seeding or complete immediately.

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use librqbit::{AddTorrent, AddTorrentResponse};
use rd_core::{DownloadFile, DownloadKind, DownloadPackage, DownloadState, Failure, FailureKind};
use rd_scheduler::{ExternalRunner, RunOutcome};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::TorrentService;

const PROGRESS_INTERVAL: Duration = Duration::from_millis(750);

/// Downloads `DownloadKind::Torrent` files.
pub struct TorrentRunner {
    service: TorrentService,
}

impl TorrentRunner {
    #[must_use]
    pub fn new(service: TorrentService) -> Self {
        Self { service }
    }
}

/// Turns a queue source into a librqbit request: magnet and http(s) URLs go straight to
/// the session, `file://` points at a stored `.torrent` file.
pub(crate) fn add_request(source: &Url) -> Result<AddTorrent<'static>> {
    if source.scheme() == "file" {
        let path = source
            .to_file_path()
            .ok()
            .context("stored .torrent path is invalid")?;
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read stored .torrent {}", path.display()))?;
        return Ok(AddTorrent::from_bytes(bytes));
    }
    Ok(AddTorrent::from_url(source.to_string()))
}

#[async_trait]
impl ExternalRunner for TorrentRunner {
    fn kind(&self) -> DownloadKind {
        DownloadKind::Torrent
    }

    fn slot_capacity(&self) -> usize {
        4
    }

    async fn run(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
        cancellation: CancellationToken,
        _limits: rd_scheduler::RunLimits,
    ) -> Result<RunOutcome> {
        let (session, generation) = match self.service.session_slot().await {
            Ok(slot) => slot,
            Err(error) => {
                return Ok(RunOutcome::Failed(Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: Some(120),
                    },
                    "torrent.session_failed",
                    format!("torrent session could not be started: {error:#}"),
                )));
            }
        };
        tokio::fs::create_dir_all(&package.destination).await?;
        let request = match add_request(&file.source) {
            Ok(request) => request,
            Err(error) => {
                return Ok(RunOutcome::Failed(Failure::coded(
                    FailureKind::Permanent,
                    "torrent.source_invalid",
                    error.to_string(),
                )));
            }
        };
        let options = self
            .service
            .add_options(file.id, &package.destination)
            .await;
        let response = match session.add_torrent(request, Some(options)).await {
            Ok(response) => response,
            Err(error) => {
                return Ok(RunOutcome::Failed(Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: Some(300),
                    },
                    "torrent.add_failed",
                    format!("{error:#}"),
                )));
            }
        };
        let (torrent_id, handle) = match response {
            AddTorrentResponse::Added(id, handle)
            | AddTorrentResponse::AlreadyManaged(id, handle) => (id, handle),
            AddTorrentResponse::ListOnly(_) => {
                return Ok(RunOutcome::Failed(Failure::coded(
                    FailureKind::Permanent,
                    "torrent.add_failed",
                    "unexpected list-only response",
                )));
            }
        };
        let id_or_hash = librqbit::api::TorrentIdOrHash::Id(torrent_id);
        let info_hash = handle.info_hash().as_string();
        // Registered before the transfer starts so the API can reach a running torrent.
        self.service.inner.registry.write().await.register(
            file.id,
            torrent_id,
            info_hash.clone(),
            crate::registry::TorrentPhase::Downloading,
            generation,
        );
        // Metadata (magnets) and initial file checks happen here; bail out on stop.
        tokio::select! {
            () = cancellation.cancelled() => {
                let _ = session.pause(&handle).await;
                return Ok(RunOutcome::Stopped);
            }
            result = handle.wait_until_initialized() => {
                if let Err(error) = result {
                    let _ = session.delete(id_or_hash, false).await;
                    return Ok(RunOutcome::Failed(Failure::coded(
                        FailureKind::Transient { retry_after_seconds: Some(300) },
                        "torrent.init_failed",
                        format!("{error:#}"),
                    )));
                }
            }
        }
        // The stored plan addresses files by index, which is only meaningful for the exact
        // metadata it was reviewed against. A different info hash means the source changed
        // underneath the row, so it is refused rather than applied to the wrong files.
        let mut state = self.service.job_state(file.id).await;
        if let Some(stored) = state.metadata.as_ref()
            && stored.info_hash != info_hash
        {
            let _ = session.delete(id_or_hash, false).await;
            self.service.inner.registry.write().await.forget(file.id);
            return Ok(RunOutcome::Failed(Failure::coded(
                FailureKind::Permanent,
                "torrent.metadata_mismatch",
                "The torrent metadata no longer matches the reviewed file list",
            )));
        }
        if state.metadata.is_none() {
            // A magnet only reveals its file tree here; store it so the detail view and a
            // later restart work from the same model as an uploaded `.torrent`.
            if let Some(metadata) = self.service.metadata_of(&handle).await {
                state.metadata = Some(metadata);
                self.service.store_job_state(file.id, state).await;
            }
        }
        let name = handle
            .name()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| file.file_name.clone());
        let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
        loop {
            tokio::select! {
                () = cancellation.cancelled() => {
                    // Pause keeps the torrent in the persisted session for a later resume.
                    let _ = session.pause(&handle).await;
                    return Ok(RunOutcome::Stopped);
                }
                result = handle.wait_until_completed() => {
                    if let Err(error) = result {
                        let _ = session.delete(id_or_hash, false).await;
                        return Ok(RunOutcome::Failed(Failure::coded(
                            FailureKind::Transient { retry_after_seconds: Some(300) },
                            "torrent.transfer_failed",
                            format!("{error:#}"),
                        )));
                    }
                    break;
                }
                _ = ticker.tick() => {
                    let stats = handle.stats();
                    if let Some(error) = stats.error {
                        let _ = session.delete(id_or_hash, false).await;
                        return Ok(RunOutcome::Failed(Failure::coded(
                            FailureKind::Transient { retry_after_seconds: Some(300) },
                            "torrent.transfer_failed",
                            error,
                        )));
                    }
                    let _ = self
                        .service
                        .inner
                        .database
                        .set_download_progress(file.id, stats.progress_bytes, Some(stats.total_bytes))
                        .await;
                    // Widens the selection once the open priority tier has finished.
                    self.service.advance_tiers(file.id, &stats.file_progress).await;
                }
            }
        }
        let stats = handle.stats();
        let _ = self
            .service
            .inner
            .database
            .set_download_progress(file.id, stats.total_bytes, Some(stats.total_bytes))
            .await;
        let seeding_enabled = self
            .service
            .inner
            .settings
            .read()
            .await
            .torrent_seeding_enabled;
        if seeding_enabled {
            crate::seeding::begin(&self.service, file.id, torrent_id, info_hash, generation).await;
            let mut state = self.service.job_state(file.id).await;
            state.seed.start(chrono::Utc::now());
            self.service.store_job_state(file.id, state).await;
            return Ok(RunOutcome::Detached {
                state: DownloadState::Seeding,
            });
        }
        let _ = session.delete(id_or_hash, false).await;
        self.service.inner.registry.write().await.forget(file.id);
        self.service.discard_stored_torrent_file(&file.source).await;
        Ok(RunOutcome::Completed { final_name: name })
    }
}
