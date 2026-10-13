//! The one removal path and the reset: a row leaves the queue with its torrent, an active file
//! is cancelled first and given a moment to let go (RD-1280-08 split it from the handlers).

use super::*;

/// Discards a file's data and queues it again from zero.
///
/// A torrent additionally leaves the persisted librqbit session, because a handle that still
/// knows the old pieces would resume them instead of fetching the data again.
pub(crate) async fn reset_download_file(
    state: &AppState,
    id: DownloadId,
    delete_completed_files: bool,
) -> anyhow::Result<()> {
    state.scheduler.reset(id, delete_completed_files).await?;
    state.torrent.forget(id).await;
    Ok(())
}

/// Removes one queue row, and with it the row's torrent from the engine session.
///
/// The one removal path: the single `DELETE`, the bulk action, package deletion — and through
/// that auto-remove, the SABnzbd and qBittorrent compatibility APIs and MCP — all end here.
/// Only the single `DELETE` used to forget the torrent, so a torrent removed any other way
/// stayed in the persisted librqbit session and had its files created again on every start
/// (RD-120-68). The torrent is located first because its info hash is stored on the row.
/// The payload stays on disk, as for every other kind of download.
pub(crate) async fn remove_download(state: &AppState, id: DownloadId) -> anyhow::Result<()> {
    remove_download_discarding(state, id, false).await
}

/// [`remove_download`], and with `discard_partial` the data an unfinished file wrote outside
/// staging goes first: tool fragments beside the target, an unfinished torrent's files
/// (RD-180-21). Data first, row last — a crash in between leaves a row that knows less data,
/// never data that no row knows. The caller decides what counts as unfinished.
async fn remove_download_discarding(
    state: &AppState,
    id: DownloadId,
    discard_partial: bool,
) -> anyhow::Result<()> {
    let torrent = state.torrent.locate(id).await;
    if discard_partial {
        state.scheduler.discard_partial(id).await?;
        state.torrent.discard_located(torrent.clone()).await;
    }
    state.scheduler.remove(id).await?;
    state.torrent.forget_located(torrent).await;
    Ok(())
}

/// How often, and how far apart, a removal is tried again while a cancelled worker lets go.
pub(super) const CANCEL_WAIT_ROUNDS: usize = 25;
pub(super) const CANCEL_WAIT_STEP: std::time::Duration = std::time::Duration::from_millis(200);

/// Cancels an active file first and waits briefly for its token to clear before removal.
///
/// `discard_partial` as in [`remove_download_discarding`].
pub(crate) async fn remove_with_cancel(
    state: &AppState,
    id: DownloadId,
    discard_partial: bool,
) -> anyhow::Result<()> {
    if cancel_if_running(state, id).await? {
        for _ in 0..CANCEL_WAIT_ROUNDS {
            tokio::time::sleep(CANCEL_WAIT_STEP).await;
            if remove_download_discarding(state, id, discard_partial)
                .await
                .is_ok()
            {
                return Ok(());
            }
        }
    }
    remove_download_discarding(state, id, discard_partial).await
}

/// Cancels the file when something works on it; answers whether it did.
pub(super) async fn cancel_if_running(state: &AppState, id: DownloadId) -> anyhow::Result<bool> {
    let running = state
        .database
        .get_download(id)
        .await?
        .is_some_and(|current| {
            matches!(
                current.state,
                rd_core::DownloadState::Resolving
                    | rd_core::DownloadState::Downloading
                    | rd_core::DownloadState::Verifying
                    | rd_core::DownloadState::Repairing
                    | rd_core::DownloadState::Extracting
            )
        });
    if running {
        state.scheduler.cancel(id).await?;
    }
    Ok(running)
}
