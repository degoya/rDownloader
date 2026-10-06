//! Queue runner: one row drives one torrent — add to the session, download into the
//! package folder, then either hand over to seeding or complete immediately.

use std::{ops::ControlFlow, sync::Arc, time::Duration};

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

    /// Pieces are hash-checked before they count, so a stopped torrent resumes from what
    /// verifies and a finished one found on disk is adopted by that same check; the torrent's
    /// own file tree is kept, so the collision policy does not rename inside it.
    fn reuse(&self) -> rd_core::ReuseCapability {
        rd_core::ReuseCapability {
            resume_partial: true,
            recheck_partial: true,
            adopt_completed: true,
            verify_completed: true,
            applies_collision_policy: false,
        }
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
        let Added {
            session,
            generation,
            torrent_id,
            handle,
        } = match self.add_to_session(file, package).await? {
            ControlFlow::Continue(added) => added,
            ControlFlow::Break(outcome) => return Ok(outcome),
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
        if let Some(outcome) = self
            .initialize(file, &session, &handle, id_or_hash, &cancellation)
            .await
        {
            return Ok(outcome);
        }
        // Every piece has been hashed now; a recheck asked for reports what it found.
        self.service.finish_recheck(file.id, &handle).await;
        if let Some(outcome) = self
            .check_metadata(file, &session, &handle, id_or_hash, &info_hash)
            .await
        {
            return Ok(outcome);
        }
        let name = handle
            .name()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| file.file_name.clone());
        if let Some(outcome) = self
            .transfer(file, &session, &handle, id_or_hash, &cancellation)
            .await
        {
            return Ok(outcome);
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

/// A torrent the session took, with the slot it lives in.
struct Added {
    session: Arc<librqbit::Session>,
    generation: u64,
    torrent_id: usize,
    handle: Arc<librqbit::ManagedTorrent>,
}

impl TorrentRunner {
    /// Adds the row's torrent to the session, or names the outcome that ends the run first.
    async fn add_to_session(
        &self,
        file: &DownloadFile,
        package: &DownloadPackage,
    ) -> Result<ControlFlow<RunOutcome, Added>> {
        // Its files are being carried to another folder right now; added here, the torrent
        // would start writing into the folder that is being emptied (RD-1100-10).
        if self.service.is_relocating(file.id).await {
            return Ok(ControlFlow::Break(RunOutcome::Failed(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: Some(60),
                },
                "torrent.relocation_running",
                "The torrent's files are being moved",
            ))));
        }
        let (session, generation) = match self.service.session_slot().await {
            Ok(slot) => slot,
            Err(error) => {
                return Ok(ControlFlow::Break(RunOutcome::Failed(Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: Some(120),
                    },
                    "torrent.session_failed",
                    format!("torrent session could not be started: {error:#}"),
                ))));
            }
        };
        tokio::fs::create_dir_all(&package.destination).await?;
        let request = match add_request(&file.source) {
            Ok(request) => request,
            Err(error) => {
                return Ok(ControlFlow::Break(RunOutcome::Failed(Failure::coded(
                    FailureKind::Permanent,
                    "torrent.source_invalid",
                    format!("{error:#}"),
                ))));
            }
        };
        let options = self
            .service
            .add_options(file.id, &package.destination)
            .await;
        let response = match session.add_torrent(request, Some(options)).await {
            Ok(response) => response,
            Err(error) => {
                return Ok(ControlFlow::Break(RunOutcome::Failed(Failure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: Some(300),
                    },
                    "torrent.add_failed",
                    format!("{error:#}"),
                ))));
            }
        };
        let (torrent_id, handle) = match response {
            AddTorrentResponse::Added(id, handle)
            | AddTorrentResponse::AlreadyManaged(id, handle) => (id, handle),
            AddTorrentResponse::ListOnly(_) => {
                return Ok(ControlFlow::Break(RunOutcome::Failed(Failure::coded(
                    FailureKind::Permanent,
                    "torrent.add_failed",
                    "unexpected list-only response",
                ))));
            }
        };
        Ok(ControlFlow::Continue(Added {
            session,
            generation,
            torrent_id,
            handle,
        }))
    }

    /// Waits for the metadata (magnets) and the initial file checks; `Some` ends the run.
    async fn initialize(
        &self,
        file: &DownloadFile,
        session: &Arc<librqbit::Session>,
        handle: &Arc<librqbit::ManagedTorrent>,
        id_or_hash: librqbit::api::TorrentIdOrHash,
        cancellation: &CancellationToken,
    ) -> Option<RunOutcome> {
        // Metadata (magnets) and initial file checks happen here; bail out on stop.
        tokio::select! {
            () = cancellation.cancelled() => {
                let _ = session.pause(handle).await;
                return Some(RunOutcome::Stopped);
            }
            result = handle.wait_until_initialized() => {
                if let Err(error) = result {
                    let _ = session.delete(id_or_hash, false).await;
                    // Out of the session, so out of the registry too (audit 1.9.1, TR-10).
                    self.service.inner.registry.write().await.forget(file.id);
                    return Some(RunOutcome::Failed(Failure::coded(
                        FailureKind::Transient { retry_after_seconds: Some(300) },
                        "torrent.init_failed",
                        format!("{error:#}"),
                    )));
                }
            }
        }
        None
    }

    /// Holds the session's metadata against the stored one and keeps a magnet's file tree;
    /// `Some` ends the run.
    async fn check_metadata(
        &self,
        file: &DownloadFile,
        session: &Arc<librqbit::Session>,
        handle: &Arc<librqbit::ManagedTorrent>,
        id_or_hash: librqbit::api::TorrentIdOrHash,
        info_hash: &str,
    ) -> Option<RunOutcome> {
        // The stored plan addresses files by index, which is only meaningful for the exact
        // metadata it was reviewed against. A different info hash means the source changed
        // underneath the row, so it is refused rather than applied to the wrong files.
        let mut state = self.service.job_state(file.id).await;
        if let Some(stored) = state.metadata.as_ref()
            && stored.info_hash != info_hash
        {
            let _ = session.delete(id_or_hash, false).await;
            self.service.inner.registry.write().await.forget(file.id);
            return Some(RunOutcome::Failed(Failure::coded(
                FailureKind::Permanent,
                "torrent.metadata_mismatch",
                "The torrent metadata no longer matches the reviewed file list",
            )));
        }
        if state.metadata.is_none() {
            // A magnet only reveals its file tree here; store it so the detail view and a
            // later restart work from the same model as an uploaded `.torrent`.
            if let Some(metadata) = self.service.metadata_of(handle).await {
                state.metadata = Some(metadata);
                self.service.store_job_state(file.id, state).await;
            }
        }
        None
    }

    /// Follows the transfer to its end, reporting progress; `Some` ends the run before it.
    async fn transfer(
        &self,
        file: &DownloadFile,
        session: &Arc<librqbit::Session>,
        handle: &Arc<librqbit::ManagedTorrent>,
        id_or_hash: librqbit::api::TorrentIdOrHash,
        cancellation: &CancellationToken,
    ) -> Option<RunOutcome> {
        let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
        loop {
            tokio::select! {
                () = cancellation.cancelled() => {
                    // Pause keeps the torrent in the persisted session for a later resume.
                    let _ = session.pause(handle).await;
                    return Some(RunOutcome::Stopped);
                }
                result = handle.wait_until_completed() => {
                    if let Err(error) = result {
                        let _ = session.delete(id_or_hash, false).await;
                        self.service.inner.registry.write().await.forget(file.id);
                        return Some(RunOutcome::Failed(Failure::coded(
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
                        self.service.inner.registry.write().await.forget(file.id);
                        return Some(RunOutcome::Failed(Failure::coded(
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
        None
    }
}
