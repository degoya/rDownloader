//! Applying the persisted file plan to the engine.
//!
//! librqbit expresses selection as `only_files`: the set of file indices it downloads. The
//! plan is resolved into exactly that set, both when a torrent is added and — through
//! `Session::update_only_files` — while it is already running.

use anyhow::{Context, Result};
use rd_core::{DownloadId, TorrentJobState, TorrentMetadataInfo};
use url::Url;

use crate::TorrentService;

impl TorrentService {
    /// Resolves the metadata behind a magnet link without downloading anything.
    ///
    /// Adds the torrent in list-only mode, which makes the engine fetch the info
    /// dictionary from the swarm and hand it back without starting a transfer. Trackers
    /// and web seeds come from the magnet itself, because they are not part of the info
    /// dictionary; the file tree is projected by the same function the `.torrent` path
    /// uses, so both sources end up with an identical model.
    pub async fn resolve_metadata(&self, source: &Url) -> Result<TorrentMetadataInfo> {
        let session = self.session().await?;
        let response = session
            .add_torrent(
                crate::runner::add_request(source)?,
                Some(librqbit::AddTorrentOptions {
                    list_only: true,
                    ..Default::default()
                }),
            )
            .await
            .context("resolve torrent metadata")?;
        match response {
            librqbit::AddTorrentResponse::ListOnly(listing) => crate::metadata::project(
                &listing.info,
                listing.info_hash.as_string(),
                listing.info.info().private,
                crate::metadata::magnet_trackers(source),
                crate::metadata::magnet_web_seeds(source),
            ),
            librqbit::AddTorrentResponse::Added(_, handle)
            | librqbit::AddTorrentResponse::AlreadyManaged(_, handle) => {
                // Already managed, so the metadata is known locally.
                let info_hash = handle.info_hash().as_string();
                handle
                    .with_metadata(|metadata| {
                        crate::metadata::project(
                            &metadata.info,
                            info_hash.clone(),
                            metadata.info.info().private,
                            crate::metadata::magnet_trackers(source),
                            crate::metadata::magnet_web_seeds(source),
                        )
                    })
                    .context("torrent metadata is not available yet")?
            }
        }
    }

    /// Persists a changed plan and pushes the new selection to a running torrent.
    ///
    /// Persisting first means a failure to reach the engine still leaves the plan stored,
    /// so the next add applies it; the row never ends up downloading files the user
    /// deselected.
    pub async fn apply_plan(&self, id: DownloadId, state: TorrentJobState) -> Result<()> {
        let selection = state.metadata.as_ref().map(|metadata| {
            rd_core::resolve_plan(metadata, &state.plan)
                .included_indices()
                .into_iter()
                .map(|index| index as usize)
                .collect::<std::collections::HashSet<usize>>()
        });
        self.inner
            .database
            .set_download_torrent_state(id, state)
            .await
            .context("persist torrent plan")?;
        let Some(selection) = selection else {
            return Ok(());
        };
        let entry = self.inner.registry.read().await.get(id).cloned();
        let Some(entry) = entry else {
            // Not running: the plan is applied the next time the torrent is added.
            return Ok(());
        };
        let session = self.session().await?;
        let Some(handle) = session.get(entry.handle()) else {
            return Ok(());
        };
        session
            .update_only_files(&handle, &selection)
            .await
            .context("apply file selection to the running torrent")
    }

    /// The domain metadata of a managed torrent, once the engine has resolved it.
    pub(crate) async fn metadata_of(
        &self,
        handle: &std::sync::Arc<librqbit::ManagedTorrent>,
    ) -> Option<TorrentMetadataInfo> {
        let info_hash = handle.info_hash().as_string();
        handle
            .with_metadata(|metadata| {
                crate::metadata::project(
                    &metadata.info,
                    info_hash.clone(),
                    metadata.info.info().private,
                    Vec::new(),
                    Vec::new(),
                )
            })
            .ok()?
            .ok()
    }

    /// The set of file indices one row should download, if it has a plan and metadata.
    pub(crate) async fn only_files(&self, id: DownloadId) -> Option<Vec<usize>> {
        let state = self.job_state(id).await;
        let metadata = state.metadata.as_ref()?;
        if state.plan.is_untouched() {
            // No user decision: let the engine download everything, which also avoids
            // pinning a stale index set if the metadata ever changes.
            return None;
        }
        // No per-file progress is known before the add, so the highest tier opens first.
        let _ = metadata;
        crate::priority::staged_selection(&state, &[])
    }
}
