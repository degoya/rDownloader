//! Removing many downloads at once (RD-1120-17): what [`super::remove_with_cancel`] does for
//! one file, with the rows going through the database writer together.
//!
//! One removal per id used to cost one writer transaction each — 500 downloads took about
//! 14 s on the server, and a browser that gave up in between left part of the selection
//! removed. The checks, the cancels and the staging files are still per file; the rows go in
//! one transaction.

use std::collections::HashSet;

use rd_core::DownloadId;
use rd_db::StoreErrorKind;

use super::{CANCEL_WAIT_ROUNDS, CANCEL_WAIT_STEP, cancel_if_running, refused};
use crate::AppState;

/// [`super::remove_with_cancel`] for a batch, keeping the partial data as the bulk action
/// always has. One answer per id, in order.
///
/// Running files are cancelled first, then every file goes in one removal. A cancelled file
/// whose worker has not let go yet is refused as still working; those are tried again
/// together, every 200 ms for up to five seconds, the single path's wait.
pub(super) async fn remove_many_with_cancel(
    state: &AppState,
    ids: &[DownloadId],
) -> Vec<anyhow::Result<()>> {
    let mut outcomes = Vec::with_capacity(ids.len());
    let mut pending = Vec::new();
    let mut cancelled = HashSet::new();
    for &id in ids {
        match cancel_if_running(state, id).await {
            Ok(running) => {
                if running {
                    cancelled.insert(id);
                }
                pending.push((outcomes.len(), id));
                outcomes.push(Ok(()));
            }
            Err(error) => outcomes.push(Err(error)),
        }
    }
    for round in 0..=CANCEL_WAIT_ROUNDS {
        if round > 0 {
            tokio::time::sleep(CANCEL_WAIT_STEP).await;
        }
        let rows: Vec<DownloadId> = pending.iter().map(|(_, id)| *id).collect();
        let answers = remove_together(state, &rows).await;
        let mut waiting = Vec::new();
        for ((index, id), answer) in pending.into_iter().zip(answers) {
            let still_letting_go = round < CANCEL_WAIT_ROUNDS
                && cancelled.contains(&id)
                && answer
                    .as_ref()
                    .is_err_and(|error| refused(error, StoreErrorKind::WrongState));
            if still_letting_go {
                waiting.push((index, id));
            } else if let Some(slot) = outcomes.get_mut(index) {
                *slot = answer;
            }
        }
        if waiting.is_empty() {
            break;
        }
        pending = waiting;
    }
    outcomes
}

/// One removal of `ids` through the scheduler; each removed file's torrent leaves the engine
/// session with it, as in [`super::remove_download`]. Located first, because the info hash is
/// stored on the row.
async fn remove_together(state: &AppState, ids: &[DownloadId]) -> Vec<anyhow::Result<()>> {
    let mut torrents = Vec::with_capacity(ids.len());
    for &id in ids {
        torrents.push(state.torrent.locate(id).await);
    }
    let answers = state.scheduler.remove_many(ids).await;
    for (torrent, answer) in torrents.into_iter().zip(&answers) {
        if answer.is_ok() {
            state.torrent.forget_located(torrent).await;
        }
    }
    answers
}
