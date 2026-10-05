//! Hashing a torrent's data again when the user asks for it (RD-1100-10).
//!
//! librqbit 9.0.1 checks pieces only while it adds a torrent, and with fastresume on it then
//! trusts the bitfield it stored beside its session after a few spot checks. It has no call that
//! checks a torrent it already holds. A recheck therefore takes the torrent out of the session —
//! the stored bitfield goes with it — and adds it again, which hashes every piece:
//!
//! - a **seed** is added again at once, in place; a piece that no longer verifies is fetched
//!   again by the live torrent, and the seeding supervisor ends no seed while its data is
//!   incomplete;
//! - **any other row** is checked the next time the runner adds it; the REST layer stops and
//!   resumes a running one, so that next time is now.
//!
//! The request and its result are kept in the row's torrent state ([`TorrentRecheck`]), so the
//! panel can say what the check found.

use anyhow::{Context, Result};
use rd_core::{DownloadId, DownloadState, TorrentRecheck};

use crate::TorrentService;

impl TorrentService {
    /// Asks for every piece of one torrent to be hashed again.
    ///
    /// `Ok(true)` when the check runs now — a seed, added again in place — and `Ok(false)` when
    /// it runs the next time the row starts.
    pub async fn recheck(&self, id: DownloadId) -> Result<bool> {
        let file = self
            .inner
            .database
            .get_download(id)
            .await?
            .context("download not found")?;
        let mut state = self.job_state(id).await;
        state.recheck = Some(TorrentRecheck::requested(chrono::Utc::now()));
        self.inner
            .database
            .set_download_torrent_state(id, state)
            .await
            .context("record the recheck")?;
        let location = self.locate(id).await;
        let info_hash = location.info_hash().map(str::to_owned);
        self.forget_located(location).await;
        if let Some(info_hash) = info_hash {
            // The session drops it with the torrent; a failed write of its list is only logged
            // there, and a bitfield left behind would let the next add trust it again.
            drop_fastresume(&self.inner.data_dir, &info_hash).await;
        }
        if file.state != DownloadState::Seeding {
            return Ok(false);
        }
        let package = self
            .inner
            .database
            .get_package(file.package_id)
            .await?
            .context("package not found")?;
        crate::seeding::resume(self, &file, &package)
            .await
            .context("add the seed again")?;
        let service = self.clone();
        tokio::spawn(async move { service.report_when_checked(id).await });
        Ok(true)
    }

    /// Writes the result of a pending recheck from a torrent the engine has just checked.
    ///
    /// Called by the runner once a torrent is initialised, and for a seed by the task the
    /// recheck spawned. Nothing happens when no recheck is pending.
    pub(crate) async fn finish_recheck(&self, id: DownloadId, handle: &librqbit::ManagedTorrent) {
        let mut state = self.job_state(id).await;
        let Some(recheck) = state
            .recheck
            .as_mut()
            .filter(|recheck| recheck.is_pending())
        else {
            return;
        };
        let stats = handle.stats();
        recheck.finished_at = Some(chrono::Utc::now());
        recheck.verified_bytes = stats.progress_bytes;
        recheck.total_bytes = stats.total_bytes;
        tracing::info!(
            download_id = %id,
            verified_bytes = stats.progress_bytes,
            total_bytes = stats.total_bytes,
            "torrent recheck finished"
        );
        self.store_job_state(id, state).await;
    }

    /// Waits until the seed added again by [`Self::recheck`] has been hashed and reports it.
    async fn report_when_checked(&self, id: DownloadId) {
        let Some(entry) = self.inner.registry.read().await.get(id).cloned() else {
            return;
        };
        let Ok(session) = self.session().await else {
            return;
        };
        let Some(handle) = session.get(entry.handle()) else {
            return;
        };
        tokio::select! {
            () = self.inner.shutdown.cancelled() => {}
            result = handle.wait_until_initialized() => match result {
                Ok(()) => self.finish_recheck(id, &handle).await,
                Err(error) => {
                    tracing::warn!(download_id = %id, %error, "torrent recheck did not finish");
                }
            },
        }
    }
}

/// Removes the fastresume bitfield librqbit keeps for one torrent beside its session.
async fn drop_fastresume(data_dir: &std::path::Path, info_hash: &str) {
    let path = crate::forget::session_folder(data_dir).join(format!("{info_hash}.bitv"));
    if let Err(error) = tokio::fs::remove_file(&path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "stored torrent bitfield could not be removed");
    }
}
