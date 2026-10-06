//! What the kill switch does to the torrents while the bound interface is gone, and once it is
//! back (RD-1120-04).
//!
//! The switch used to pause every torrent once, on the check that found the interface gone,
//! and drop whatever the engine answered. A pause that failed left that torrent talking past
//! the tunnel, and the switch, engaged already, never looked at it again. Now every check of an
//! engaged switch pauses each torrent that still carries traffic and names every one that
//! refuses in the log, so a failed pause is tried again one check interval later — and a
//! torrent that runs again meanwhile (re-added after a session rebuild, resumed by a rebuild
//! that failed) is caught by the same check.
//!
//! The torrents the switch paused are remembered, so the release resumes exactly those, not one
//! the user had stopped, and keeps one whose resume failed for the next check. A session rebuild
//! uses the same two steps for the pause around it.
//!
//! The switch reads the running session and never builds one: without a session no torrent
//! carries traffic, and building one to pause nothing was the silent early return before.

use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering},
};

use async_trait::async_trait;
use rd_core::DownloadId;

use crate::{TorrentService, registry::TorrentEntry};

/// Who pauses, in the log line of a torrent that refused.
const KILL_SWITCH: &str = "kill switch";

/// One torrent as the kill switch sees it. The session's torrents are the only implementation
/// outside the tests; the trait lets a test hand over a torrent whose pause fails.
#[async_trait]
pub(crate) trait Switched: Send + Sync {
    fn info_hash(&self) -> String;
    /// Whether the torrent may be exchanging data now: neither paused nor failed.
    fn carries_traffic(&self) -> bool;
    fn paused(&self) -> bool;
    async fn pause(&self) -> anyhow::Result<()>;
    async fn resume(&self) -> anyhow::Result<()>;
}

/// A torrent of the running session.
pub(crate) struct SessionTorrent {
    session: Arc<librqbit::Session>,
    handle: Arc<librqbit::ManagedTorrent>,
}

#[async_trait]
impl Switched for SessionTorrent {
    fn info_hash(&self) -> String {
        self.handle.info_hash().as_string()
    }

    fn carries_traffic(&self) -> bool {
        !self.handle.is_paused()
            && self.handle.with_state(|state| {
                matches!(
                    state,
                    librqbit::ManagedTorrentState::Initializing(_)
                        | librqbit::ManagedTorrentState::Live(_)
                )
            })
    }

    fn paused(&self) -> bool {
        self.handle.is_paused()
    }

    async fn pause(&self) -> anyhow::Result<()> {
        self.session.pause(&self.handle).await
    }

    async fn resume(&self) -> anyhow::Result<()> {
        self.session.unpause(&self.handle).await
    }
}

/// The registered torrents `session` holds.
pub(crate) fn session_torrents(
    session: &Arc<librqbit::Session>,
    registered: Vec<(DownloadId, TorrentEntry)>,
) -> Vec<(DownloadId, SessionTorrent)> {
    registered
        .into_iter()
        .filter_map(|(download_id, entry)| {
            session.get(entry.handle()).map(|handle| {
                (
                    download_id,
                    SessionTorrent {
                        session: session.clone(),
                        handle,
                    },
                )
            })
        })
        .collect()
}

/// Pauses every torrent that still carries traffic and adds it to `held`. One that refuses is
/// named in the log and stays out of `held`, still carrying traffic, for the caller to try again.
pub(crate) async fn hold_torrents<T: Switched>(
    torrents: &[(DownloadId, T)],
    held: &mut HashSet<DownloadId>,
    by: &'static str,
) {
    for (download_id, torrent) in torrents {
        if !torrent.carries_traffic() {
            continue;
        }
        match torrent.pause().await {
            Ok(()) => {
                held.insert(*download_id);
            }
            Err(error) => tracing::warn!(
                %download_id,
                info_hash = %torrent.info_hash(),
                by,
                error = %format!("{error:#}"),
                "a torrent could not be paused"
            ),
        }
    }
}

/// Resumes every torrent in `held` and takes it out. One that refuses is named in the log and
/// stays in `held` for the caller to try again; one that left the session, or runs again
/// already, is taken out without a resume.
pub(crate) async fn resume_held<T: Switched>(
    torrents: &[(DownloadId, T)],
    held: &mut HashSet<DownloadId>,
    by: &'static str,
) {
    held.retain(|id| torrents.iter().any(|(present, _)| present == id));
    for (download_id, torrent) in torrents {
        if !held.contains(download_id) {
            continue;
        }
        if !torrent.paused() {
            held.remove(download_id);
            continue;
        }
        match torrent.resume().await {
            Ok(()) => {
                held.remove(download_id);
            }
            Err(error) => tracing::warn!(
                %download_id,
                info_hash = %torrent.info_hash(),
                by,
                error = %format!("{error:#}"),
                "a paused torrent could not be resumed"
            ),
        }
    }
}

impl TorrentService {
    /// One check with the bound interface gone: engages the switch on the first, and on every
    /// one pauses what still carries traffic, so a pause that failed is tried again.
    pub(crate) async fn engage_kill_switch(&self) {
        if !self.inner.kill_switch_engaged.swap(true, Ordering::SeqCst) {
            tracing::warn!("bound network interface disappeared; pausing all torrent traffic");
        }
        let torrents = self.switched_torrents().await;
        let mut held = self.inner.kill_switch_held.lock().await;
        hold_torrents(&torrents, &mut held, KILL_SWITCH).await;
    }

    /// One check with the interface back, or the switch or the binding off: resumes exactly
    /// what the switch paused. A resume that failed is tried again at the next check.
    pub(crate) async fn release_kill_switch(&self) {
        if self.inner.kill_switch_engaged.swap(false, Ordering::SeqCst) {
            tracing::info!("bound network interface returned; resuming torrent traffic");
        }
        let mut held = self.inner.kill_switch_held.lock().await;
        if held.is_empty() {
            return;
        }
        let torrents = self.switched_torrents().await;
        resume_held(&torrents, &mut held, KILL_SWITCH).await;
    }

    /// The registered torrents of the running session; none when no session runs.
    async fn switched_torrents(&self) -> Vec<(DownloadId, SessionTorrent)> {
        let Some(session) = self
            .inner
            .session
            .read()
            .await
            .as_ref()
            .map(|slot| slot.session.clone())
        else {
            return Vec::new();
        };
        let registered = self.inner.registry.read().await.snapshot();
        session_torrents(&session, registered)
    }
}

#[cfg(test)]
#[path = "kill_switch_tests.rs"]
mod tests;
