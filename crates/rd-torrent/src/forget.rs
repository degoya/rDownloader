//! Taking a torrent out of the engine for good (RD-120-68).
//!
//! librqbit persists its session and restores every torrent in it on the next start, paused
//! ones included, and restoring means creating the torrent's folder and files. A queue row
//! that is removed therefore has to leave the session too, or its files come back on every
//! start. Three pieces make that hold:
//!
//! - [`TorrentService::locate`] reads the info hash while the row still exists; the job state
//!   that carries it is a column of the row and goes with it.
//! - [`TorrentService::forget_located`] removes the torrent by that hash — from the live
//!   session, or, before one has been built since the start, from the persisted list.
//! - [`drop_orphans`] runs before librqbit reads the persisted list at all and removes every
//!   torrent no queue row claims, so an orphan left by an older build is never initialised.
//!
//! None of them deletes payload: removing a download keeps what it already wrote, as the
//! remove path does for every other kind (only "reset with files" deletes data).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rd_core::{DownloadId, DownloadKind, TorrentMetadataInfo};
use serde_json::Value;
use url::Url;

use crate::{MAX_TORRENT_BYTES, ServiceInner, TorrentService, parse_torrent};

/// Where librqbit keeps its session below the service data directory.
pub(crate) fn session_folder(data_dir: &Path) -> PathBuf {
    data_dir.join("torrent-session")
}

/// A row's torrent, found while the row still existed.
#[derive(Clone, Debug)]
pub struct TorrentLocation {
    download_id: DownloadId,
    /// Lowercase hex, as librqbit writes it; `None` when the row is no torrent or its hash
    /// cannot be known without asking the network.
    info_hash: Option<String>,
}

impl TorrentLocation {
    /// The info hash the torrent was found under, if any.
    #[must_use]
    pub fn info_hash(&self) -> Option<&str> {
        self.info_hash.as_deref()
    }
}

impl TorrentService {
    /// Finds the torrent behind a queue row, for [`Self::forget_located`].
    ///
    /// Call it *before* the row is removed. The registry only knows torrents added since the
    /// start, so after a restart the hash comes from the row itself: the stored metadata, the
    /// magnet's `btih`, or the stored `.torrent` file.
    pub async fn locate(&self, id: DownloadId) -> TorrentLocation {
        let registered = self
            .inner
            .registry
            .read()
            .await
            .get(id)
            .map(|entry| entry.info_hash.clone());
        let info_hash = match registered {
            Some(hash) => Some(hash),
            None => match self.inner.database.get_download(id).await {
                Ok(Some(file)) if file.kind == DownloadKind::Torrent => {
                    let state = self.job_state(id).await;
                    info_hash_of(&file.source, state.metadata.as_ref()).await
                }
                Ok(_) => None,
                Err(error) => {
                    tracing::warn!(download_id = %id, %error, "reading the download to forget its torrent failed");
                    None
                }
            },
        };
        TorrentLocation {
            download_id: id,
            info_hash: info_hash.map(|hash| hash.to_ascii_lowercase()),
        }
    }

    /// Removes a located torrent from the engine and forgets it; the payload stays on disk.
    ///
    /// Does nothing for a row that is no torrent. Before a session has been built, the
    /// torrent is struck from the persisted list instead of building one just to delete from
    /// it — building would restore, and so create the files of, every torrent in the list.
    pub async fn forget_located(&self, location: TorrentLocation) {
        let entry = self
            .inner
            .registry
            .write()
            .await
            .forget(location.download_id);
        let Some(info_hash) = location
            .info_hash
            .or_else(|| entry.map(|entry| entry.info_hash.to_ascii_lowercase()))
        else {
            return;
        };
        let Ok(id20) = info_hash.parse::<librqbit::dht::Id20>() else {
            tracing::warn!(%info_hash, "torrent to forget has an unusable info hash");
            return;
        };
        // The write lock keeps a session from being built while the persisted list is edited.
        let guard = self.inner.session.write().await;
        if let Some(slot) = guard.as_ref() {
            let session = slot.session.clone();
            drop(guard);
            if let Err(error) = session
                .delete(librqbit::api::TorrentIdOrHash::Hash(id20), false)
                .await
            {
                // Not in the session is the expected answer for a row that never ran.
                tracing::debug!(%info_hash, %error, "torrent was not in the session");
            }
            return;
        }
        let folder = session_folder(&self.inner.data_dir);
        if let Err(error) = prune_persisted(&folder, |hash| hash != info_hash).await {
            tracing::warn!(%info_hash, %error, "torrent could not be struck from the persisted session");
        }
        drop(guard);
    }

    /// Removes one row's torrent from the session and forgets it; the payload stays on disk.
    ///
    /// For a row that stays in the queue (a reset). A row that is being removed is located
    /// before and forgotten after the removal, since its hash goes with it.
    pub async fn forget(&self, id: DownloadId) {
        let location = self.locate(id).await;
        self.forget_located(location).await;
    }
}

/// The info hash of a torrent row without asking the network.
async fn info_hash_of(source: &Url, metadata: Option<&TorrentMetadataInfo>) -> Option<String> {
    if let Some(metadata) = metadata {
        return Some(metadata.info_hash.clone());
    }
    match source.scheme() {
        "magnet" => librqbit::Magnet::parse(source.as_str())
            .ok()
            .and_then(|magnet| magnet.as_id20())
            .map(|id| id.as_string()),
        "file" => {
            let path = source.to_file_path().ok()?;
            let bytes = tokio::fs::read(&path).await.ok()?;
            (bytes.len() <= MAX_TORRENT_BYTES)
                .then(|| parse_torrent(&bytes).ok())
                .flatten()
                .map(|parsed| parsed.info_hash)
        }
        // An http(s) `.torrent` whose metadata was never stored cannot be matched. Dropping
        // its session entry is harmless: the row re-adds it from the source, and librqbit's
        // initial check finds the pieces already on disk.
        _ => None,
    }
}

/// The info hashes of every torrent row in the queue.
async fn wanted_hashes(inner: &ServiceInner) -> Result<HashSet<String>> {
    let downloads = inner.database.list_downloads().await?;
    let states: std::collections::HashMap<DownloadId, rd_core::TorrentJobState> = inner
        .database
        .all_download_torrent_states()
        .await?
        .into_iter()
        .collect();
    let mut wanted = HashSet::new();
    for file in downloads
        .iter()
        .filter(|file| file.kind == DownloadKind::Torrent)
    {
        let metadata = states
            .get(&file.id)
            .and_then(|state| state.metadata.as_ref());
        if let Some(hash) = info_hash_of(&file.source, metadata).await {
            wanted.insert(hash.to_ascii_lowercase());
        }
    }
    Ok(wanted)
}

/// Strikes every persisted torrent no queue row claims, before librqbit loads the list.
///
/// librqbit 9.0.1 offers no hook for this: `SessionPersistenceConfig` is `Json` or
/// `Postgres`, and `Session::new_with_opts` restores every entry through `add_torrent`,
/// which initialises the storage — creating the folder and the files — paused or not.
/// Deleting after the start would come too late for exactly that reason, so the list is
/// edited while no session reads it. A failure leaves the list as it is and is logged: the
/// engine must still start.
pub(crate) async fn drop_orphans(inner: &ServiceInner) {
    let wanted = match wanted_hashes(inner).await {
        Ok(wanted) => wanted,
        Err(error) => {
            tracing::warn!(%error, "queue could not be read; the persisted torrent session is loaded unchanged");
            return;
        }
    };
    match prune_persisted(&session_folder(&inner.data_dir), |hash| {
        wanted.contains(hash)
    })
    .await
    {
        Ok(removed) => {
            for info_hash in removed {
                tracing::info!(%info_hash, "torrent without a queue row dropped from the session before it could be restored");
            }
        }
        Err(error) => {
            tracing::warn!(%error, "persisted torrent session could not be cleaned up");
        }
    }
}

/// Removes from librqbit's `session.json` every torrent `keep` rejects, together with the
/// `.torrent` and `.bitv` files librqbit keeps beside it. Returns the removed hashes.
///
/// Only the entries are touched: every other field stays as librqbit wrote it, and the file
/// is replaced the way librqbit replaces it, through a temporary file and a rename.
pub(crate) async fn prune_persisted(
    folder: &Path,
    keep: impl Fn(&str) -> bool,
) -> Result<Vec<String>> {
    let path = folder.join("session.json");
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", path.display()));
        }
    };
    let mut document: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let Some(torrents) = document.get_mut("torrents").and_then(Value::as_object_mut) else {
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    torrents.retain(|_, torrent| {
        let Some(hash) = torrent
            .get("info_hash")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
        else {
            return true;
        };
        if keep(&hash) {
            true
        } else {
            removed.push(hash);
            false
        }
    });
    if removed.is_empty() {
        return Ok(removed);
    }
    let temporary = folder.join("session.json.rdownloader.tmp");
    tokio::fs::write(&temporary, serde_json::to_vec(&document)?)
        .await
        .with_context(|| format!("write {}", temporary.display()))?;
    tokio::fs::rename(&temporary, &path)
        .await
        .with_context(|| format!("replace {}", path.display()))?;
    for hash in &removed {
        for extension in ["torrent", "bitv"] {
            let file = folder.join(format!("{hash}.{extension}"));
            if let Err(error) = tokio::fs::remove_file(&file).await
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(file = %file.display(), %error, "removing a persisted torrent file failed");
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
#[path = "forget_tests.rs"]
mod tests;
