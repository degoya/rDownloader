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
    let response = service
        .add_to(&session, crate::runner::add_request(&file.source)?, options)
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
        nudge(&self.inner.seeding_nudge);
    }
}

/// Wakes the one supervisor, or, while it is in the middle of a pass, keeps the wake-up for its
/// next wait (RD-1240-28).
///
/// `notify_waiters` reached a waiting supervisor only: two torrents that finished together
/// nudged it twice, the second time while it was still checking the first seed, and that
/// nudge was lost -- both seeded until the next 30-second tick despite a limit of one.
fn nudge(signal: &tokio::sync::Notify) {
    signal.notify_one();
}

/// Whether a torrent whose download just finished goes on seeding: by its effective policy, the
/// one the supervisor ends seeds by, which is off whenever sharing is (RD-1240-28). The runner
/// read the global seeding switch alone, so with sharing off a finished torrent entered
/// "Seeding" until the supervisor's next pass ended it.
pub(crate) async fn seeds_after_download(service: &TorrentService, id: DownloadId) -> bool {
    service.effective_policy(id).await.enabled
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
    // A stop here leaves the row `seeding` with its seed time closed: the next start takes the
    // seed up again and counts that time once (RD-180-12, recovery matrix).
    rd_core::failpoint!("torrent.before_seed_completed", || anyhow::anyhow!(
        "crash point"
    ));
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
        let candidates: Vec<(DownloadId, TorrentEntry)> = service
            .inner
            .registry
            .read()
            .await
            .in_phase(TorrentPhase::Seeding);
        // Nothing seeds: no pass, and above all no engine built for one. Building it would
        // restore the persisted list, and every removal after that goes through the live
        // session instead of striking the persisted entry (every settings save nudges here).
        if candidates.is_empty() {
            continue;
        }
        let Ok(session) = service.session().await else {
            continue;
        };
        // What is still seeding after the ratio and time checks, for the seed limit below.
        let mut active = candidates.len();
        let mut finished = Vec::new();
        for (download_id, entry) in candidates {
            let Some(handle) = session.get(entry.handle()) else {
                // Torrent vanished from the session; complete the row so it never hangs.
                let _ = stop(&service, download_id, "torrent left the session").await;
                active -= 1;
                continue;
            };
            let stats = handle.stats();
            if !stats.finished {
                // Its data is being hashed — after a restart, a recheck or a move — or a recheck
                // found pieces it is fetching again (RD-1100-10). No limit ends a seed with
                // incomplete data; a manual stop still does.
                let _ = service
                    .inner
                    .database
                    .set_download_progress(
                        download_id,
                        stats.progress_bytes,
                        Some(stats.total_bytes),
                    )
                    .await;
                continue;
            }
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
                active -= 1;
            } else {
                finished.push((download_id, seeded_seconds));
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
        end_surplus_seeds(&service, active, finished).await;
    }
}

/// Ends the seeds past the "active seeds" limit, the longest seeded first (RD-1240-16).
async fn end_surplus_seeds(
    service: &TorrentService,
    active: usize,
    finished: Vec<(DownloadId, u64)>,
) {
    let limit = service.inner.settings.read().await.torrent_max_active_seeds;
    for download_id in crate::limits::seeds_over_limit(active, finished, limit) {
        if let Err(error) = stop(service, download_id, "seed limit reached").await {
            tracing::warn!(download_id = %download_id, %error, "seed stop failed");
        }
    }
}

#[cfg(all(test, feature = "failpoints"))]
#[path = "seeding_crash_tests.rs"]
mod crash_tests;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rd_core::{CandidateId, DownloadId};

    use super::{nudge, seeds_after_download};
    use crate::TorrentService;

    /// RD-1240-28: a nudge while the supervisor is busy is kept for its next wait.
    #[tokio::test]
    async fn a_nudge_while_nobody_waits_is_kept() {
        let signal = tokio::sync::Notify::new();
        nudge(&signal);
        tokio::time::timeout(Duration::from_secs(1), signal.notified())
            .await
            .expect("the nudge was kept for the next wait");
    }

    #[tokio::test]
    async fn a_finished_torrent_seeds_only_while_sharing_is_on() {
        let scratch = std::env::temp_dir().join(format!("rd-torrent-seeds-{}", CandidateId::new()));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let database = rd_db::Database::open(scratch.join("torrent.sqlite3"))
            .await
            .expect("database");
        let settings = crate::shared_settings(&database).await.expect("settings");
        let service = TorrentService::start(
            database,
            settings,
            scratch.clone(),
            scratch.join("downloads"),
        );
        let id = DownloadId::new();
        {
            let mut settings = service.inner.settings.write().await;
            settings.torrent_seeding_enabled = true;
            settings.torrent_sharing_enabled = false;
        }
        assert!(!seeds_after_download(&service, id).await);
        service.inner.settings.write().await.torrent_sharing_enabled = true;
        assert!(seeds_after_download(&service, id).await);
        service.inner.settings.write().await.torrent_seeding_enabled = false;
        assert!(!seeds_after_download(&service, id).await);
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
