//! Seeding supervision: finished torrents keep uploading until the configured ratio or
//! time limit is reached (or the user stops them), then their queue row completes.

use anyhow::{Context, Result};
use rd_core::{
    DownloadFile, DownloadId, DownloadPackage, DownloadState, EffectiveSeedingPolicy,
    resolve_seeding_policy,
};

use crate::{
    TorrentService,
    registry::{TorrentEntry, TorrentPhase},
};

const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Moves a finished torrent into seeding supervision.
///
/// The runner already registered the row while it was downloading, so the common path only
/// flips the phase; the resume path after a restart registers it from scratch.
pub(crate) async fn begin(
    service: &TorrentService,
    download_id: DownloadId,
    torrent_id: usize,
    info_hash: String,
    generation: u64,
) {
    let mut registry = service.inner.registry.write().await;
    if registry.get(download_id).is_some() {
        registry.set_phase(download_id, TorrentPhase::Seeding);
    } else {
        registry.register(
            download_id,
            torrent_id,
            info_hash,
            TorrentPhase::Seeding,
            generation,
        );
    }
}

/// Re-adds one seeding row's torrent after a restart.
pub(crate) async fn resume(
    service: &TorrentService,
    file: &DownloadFile,
    package: &DownloadPackage,
) -> Result<()> {
    let (session, generation) = service.session_slot().await?;
    let options = service.add_options(file.id, &package.destination).await;
    let response = session
        .add_torrent(crate::runner::add_request(&file.source)?, Some(options))
        .await
        .context("re-add seeding torrent")?;
    match response {
        librqbit::AddTorrentResponse::Added(id, handle)
        | librqbit::AddTorrentResponse::AlreadyManaged(id, handle) => {
            let info_hash = handle.info_hash().as_string();
            begin(service, file.id, id, info_hash, generation).await;
            Ok(())
        }
        librqbit::AddTorrentResponse::ListOnly(_) => anyhow::bail!("unexpected list-only response"),
    }
}

impl TorrentService {
    /// The seeding policy that applies to one queue row.
    ///
    /// Global settings, then the category of its package, then the torrent's own override;
    /// each field inherits independently so a category can raise the ratio while the seed
    /// time still comes from the global settings.
    pub async fn effective_policy(&self, id: DownloadId) -> EffectiveSeedingPolicy {
        let settings = self.inner.settings.read().await.clone();
        let torrent = self.job_state(id).await.seeding;
        let category = self.category_policy(id).await;
        resolve_seeding_policy(&settings, category.as_ref(), Some(&torrent))
    }

    /// The seeding override of the category the row's package belongs to.
    async fn category_policy(&self, id: DownloadId) -> Option<rd_core::SeedingPolicyOverride> {
        let file = self.inner.database.get_download(id).await.ok()??;
        let packages = self.inner.database.list_packages().await.ok()?;
        let category = packages
            .iter()
            .find(|package| package.id == file.package_id)?
            .category_id?;
        self.inner
            .database
            .list_categories()
            .await
            .ok()?
            .into_iter()
            .find(|candidate| candidate.id == category)?
            .seeding
    }

    /// Closes the seed clock of one row, folding the running stretch into the total.
    pub(crate) async fn close_seed_clock(&self, id: DownloadId) {
        let mut state = self.job_state(id).await;
        state.seed.stop(chrono::Utc::now());
        self.store_job_state(id, state).await;
    }

    /// Wakes the supervisor so a changed policy takes effect at once.
    pub fn nudge_seeding(&self) {
        self.inner.seeding_nudge.notify_waiters();
    }
}

/// Stops one seed and completes its queue row; `Ok(false)` when it was not seeding.
pub(crate) async fn stop(service: &TorrentService, id: DownloadId, reason: &str) -> Result<bool> {
    let entry = {
        let mut registry = service.inner.registry.write().await;
        match registry.get(id) {
            Some(entry) if entry.phase == TorrentPhase::Seeding => registry.forget(id),
            _ => None,
        }
    };
    let Some(entry) = entry else {
        return Ok(false);
    };
    if let Ok(session) = service.session().await {
        // Keep the payload files; only forget the torrent.
        let _ = session.delete(entry.handle(), false).await;
    }
    // Fold the running stretch into the total before the row leaves seeding, so a later
    // restart cannot double-count or reset it.
    service.close_seed_clock(id).await;
    service
        .inner
        .database
        .transition_download(id, DownloadState::Completed)
        .await?;
    if let Ok(Some(file)) = service.inner.database.get_download(id).await {
        service.discard_stored_torrent_file(&file.source).await;
    }
    tracing::info!(download_id = %id, reason, "seeding finished");
    Ok(true)
}

/// Background loop: checks every seed against the ratio and time limits.
///
/// Woken either by the interval or by [`ServiceInner::seeding_nudge`], so a limit lowered
/// through the API takes effect at once rather than up to one interval later.
pub(crate) async fn supervise(service: TorrentService) {
    loop {
        tokio::select! {
            () = service.inner.shutdown.cancelled() => return,
            () = tokio::time::sleep(CHECK_INTERVAL) => {}
            () = service.inner.seeding_nudge.notified() => {}
        }
        let Ok(session) = service.session().await else {
            continue;
        };
        let candidates: Vec<(DownloadId, TorrentEntry)> = service
            .inner
            .registry
            .read()
            .await
            .in_phase(TorrentPhase::Seeding);
        for (download_id, entry) in candidates {
            let Some(handle) = session.get(entry.handle()) else {
                // Torrent vanished from the session; complete the row so it never hangs.
                let _ = stop(&service, download_id, "torrent left the session").await;
                continue;
            };
            let stats = handle.stats();
            // Read per torrent, not once per tick: a category or torrent override can
            // differ from the global settings and from every other seed in the loop.
            let policy = service.effective_policy(download_id).await;
            let mut state = service.job_state(download_id).await;
            state.seed.start(chrono::Utc::now());
            let seeded_seconds = state.seed.seeded_seconds(chrono::Utc::now());
            service.store_job_state(download_id, state).await;
            let ratio_reached = policy.ratio_reached(stats.uploaded_bytes, stats.total_bytes);
            let time_reached = policy.time_reached(seeded_seconds);
            if !policy.enabled || ratio_reached || time_reached {
                let reason = if !policy.enabled {
                    "seeding disabled by policy"
                } else if ratio_reached {
                    "ratio reached"
                } else {
                    "time limit reached"
                };
                if let Err(error) = stop(&service, download_id, reason).await {
                    tracing::warn!(download_id = %download_id, %error, "seed stop failed");
                }
            } else {
                // Surface the upload progress: committed stays at the payload size, and
                // the UI derives the ratio from uploaded bytes in the stats endpoint later.
                let _ = service
                    .inner
                    .database
                    .set_download_progress(
                        download_id,
                        stats.progress_bytes,
                        Some(stats.total_bytes),
                    )
                    .await;
            }
        }
    }
}
