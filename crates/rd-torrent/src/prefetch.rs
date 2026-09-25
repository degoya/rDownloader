//! The `.torrent` a link check already read, kept for the download (RD-130-18).
//!
//! Since RD-120-68 the check reads a link that answers as a torrent, to name the package
//! after `info.name`, and the download then fetched the same file a second time. An indexer
//! that counts or limits grabs counted two. The bytes the check read are kept here instead,
//! and when the candidate is queued they are stored the way an uploaded torrent is, so the
//! queue row gets the same `file://` source and the engine never asks the address again.
//!
//! Kept per candidate rather than per info hash: two links to the same torrent are two
//! candidates, and removing one must not take the file the other is waiting on. The copy a
//! queue row points at is stored by info hash, like every other stored torrent.
//!
//! Expiry is by age and by the candidate going away, never by a restart: a file on disk
//! survives one, and a torrent is identified by its info hash, so the bytes cannot go stale.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use rd_core::CandidateId;

use crate::{MAX_TORRENT_BYTES, TorrentService, parse_torrent};

/// How long a torrent read at the check is kept for its download.
///
/// Long enough for a link that is reviewed before it is queued, short enough that a
/// LinkGrabber left full for weeks does not keep every file it ever checked. A candidate
/// queued after this downloads the file again, as it did before RD-130-18.
pub const PREFETCH_TTL: Duration = Duration::from_secs(24 * 60 * 60);

impl TorrentService {
    fn prefetch_directory(&self) -> PathBuf {
        self.torrent_file_directory().join("prefetched")
    }

    fn prefetch_path(&self, candidate: CandidateId) -> PathBuf {
        self.prefetch_directory()
            .join(format!("{candidate}.torrent"))
    }

    /// Keeps the bytes the check read for the candidate's download.
    ///
    /// Bounded by the same 16 MiB an upload is; the check never reads more, so the bound
    /// only guards a caller that did.
    pub async fn keep_prefetched(&self, candidate: CandidateId, bytes: &[u8]) -> Result<()> {
        anyhow::ensure!(
            bytes.len() <= MAX_TORRENT_BYTES,
            "torrent file exceeds the 16 MiB limit"
        );
        let directory = self.prefetch_directory();
        tokio::fs::create_dir_all(&directory)
            .await
            .with_context(|| format!("create torrent directory {}", directory.display()))?;
        let path = self.prefetch_path(candidate);
        tokio::fs::write(&path, bytes)
            .await
            .with_context(|| format!("keep checked torrent {}", path.display()))
    }

    /// Stores the candidate's kept torrent where an uploaded one lives and returns that path.
    ///
    /// `None` when there is nothing to reuse: nothing was kept, it has expired, or it is not
    /// the torrent the candidate was reviewed with. The caller then leaves the address as the
    /// source and the download fetches it again. The kept file itself stays until
    /// [`TorrentService::prune_prefetched`] sees the candidate queued, so an enqueue that
    /// fails further on can be retried without a second fetch.
    pub async fn promote_prefetched(
        &self,
        candidate: CandidateId,
        info_hash: &str,
    ) -> Result<Option<PathBuf>> {
        let path = self.prefetch_path(candidate);
        let modified = match tokio::fs::metadata(&path).await {
            Ok(metadata) => metadata
                .modified()
                .with_context(|| format!("read the age of {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", path.display()));
            }
        };
        if expired(modified, SystemTime::now()) {
            remove_quietly(&path).await;
            return Ok(None);
        }
        let bytes = tokio::fs::read(&path)
            .await
            .with_context(|| format!("read checked torrent {}", path.display()))?;
        if !parse_torrent(&bytes)?
            .info_hash
            .eq_ignore_ascii_case(info_hash)
        {
            return Ok(None);
        }
        let (_, stored) = self.store_torrent_file(&bytes).await?;
        Ok(Some(stored))
    }

    /// Removes every kept torrent whose candidate is no longer open — removed or queued —
    /// and every one older than [`PREFETCH_TTL`].
    ///
    /// `open` is the set of candidates that may still be queued. A file whose name is not a
    /// candidate id is removed as well: nothing else writes into this directory.
    pub async fn prune_prefetched(&self, open: &HashSet<CandidateId>) {
        let Ok(mut entries) = tokio::fs::read_dir(self.prefetch_directory()).await else {
            return;
        };
        let now = SystemTime::now();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let owner = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse::<CandidateId>().ok());
            let fresh = entry
                .metadata()
                .await
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .is_some_and(|modified| !expired(modified, now));
            if fresh && owner.is_some_and(|id| open.contains(&id)) {
                continue;
            }
            remove_quietly(&path).await;
        }
    }
}

fn expired(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age > PREFETCH_TTL)
}

async fn remove_quietly(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "removing a kept torrent file failed");
    }
}

#[cfg(test)]
#[path = "prefetch_tests.rs"]
mod tests;
