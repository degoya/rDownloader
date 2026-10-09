//! The queue's stop mark (RD-1210-02), JDownloader's behaviour: "stop after this".
//!
//! A mark sits on one file or one package, never on a position, so it moves with its target when
//! the queue is reordered and goes with it when the target is deleted (the row's foreign keys).
//! Once the target is done — a file completed or failed for good, a package with no file left
//! waiting or running — the supervise loop holds the queue until somebody resumes it: what waits
//! is paused, what runs finishes. The mark is cleared, and `queue.stop_mark` says it was reached,
//! which is what a notification rule hangs off.
//!
//! The check runs every tick, before the dispatch pass of the same tick, and the dispatcher only
//! ever starts files from that pass, so a target finishing never lets the next file start first.
//!
//! The pause is recorded before the mark is cleared. A stop between the two
//! (`scheduler.after_stop_mark_paused`) leaves both: the next start restores the hold before
//! its first dispatch, finds the mark on a finished target and acts on it again, which changes
//! nothing about the pause and clears the mark; the event goes out once, after the clear.

use anyhow::{Result, bail};
use rd_core::{DownloadState, EventEnvelope, EventKind};
use rd_db::{StopMark, StopMarkTarget, StoreError};

use crate::SchedulerHandle;

/// How far a stop mark's target has got.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Progress {
    /// Something of it still waits, is paused or held, or runs.
    Ahead,
    /// Nothing of it is left to do.
    Done,
    /// The file or the package no longer exists.
    Gone,
}

/// A file in this state is done as far as a stop mark is concerned: completed, failed for good,
/// cancelled, stood down as a mirror, or downloaded and seeding. Paused, blocked and retrying
/// files still lie ahead.
const fn done(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Completed
            | DownloadState::Failed
            | DownloadState::Cancelled
            | DownloadState::Skipped
            | DownloadState::Seeding
    )
}

impl SchedulerHandle {
    /// The stop mark in force, if any.
    pub async fn stop_mark(&self) -> Result<Option<StopMark>> {
        self.database.stop_mark().await
    }

    /// Sets the stop mark on `target`, replacing the one in force. Refused with a
    /// [`StoreError`] when the target does not exist (`NotFound`) or is already done
    /// (`WrongState`): a mark there would stop the queue at once.
    pub async fn set_stop_mark(&self, target: StopMarkTarget) -> Result<StopMark> {
        match self.progress(target).await? {
            Progress::Gone => bail!(StoreError::not_found("no such file or package")),
            Progress::Done => bail!(StoreError::wrong_state(
                "the stop mark's file or package is already finished"
            )),
            Progress::Ahead => {}
        }
        let mark = self.database.set_stop_mark(target).await?;
        self.announce_stop_mark("set", target);
        Ok(mark)
    }

    /// Removes the stop mark; answers whether one was set.
    pub async fn clear_stop_mark(&self) -> Result<bool> {
        let Some(mark) = self.database.stop_mark().await? else {
            return Ok(false);
        };
        let cleared = self.database.clear_stop_mark(Some(mark.target)).await?;
        if cleared {
            self.announce_stop_mark("cleared", mark.target);
        }
        Ok(cleared)
    }

    /// One supervision step: acts on the mark once its target is done, and forgets a mark whose
    /// target vanished between two reads.
    pub(crate) async fn supervise_stop_mark(&self) -> Result<()> {
        let Some(mark) = self.database.stop_mark().await? else {
            return Ok(());
        };
        match self.progress(mark.target).await? {
            Progress::Ahead => Ok(()),
            Progress::Gone => {
                self.database.clear_stop_mark(Some(mark.target)).await?;
                Ok(())
            }
            Progress::Done => self.reach_stop_mark(mark).await,
        }
    }

    async fn reach_stop_mark(&self, mark: StopMark) -> Result<()> {
        let pause = self.pause_queue_until_resumed().await?;
        rd_core::failpoint!("scheduler.after_stop_mark_paused", || anyhow::anyhow!(
            "crash point scheduler.after_stop_mark_paused"
        ));
        // Only the mark acted on: one somebody set a moment ago on another target stays.
        if self.database.clear_stop_mark(Some(mark.target)).await? {
            tracing::info!(
                files = pause.files.len(),
                "the stop mark was reached; the queue is paused"
            );
            self.announce_stop_mark("reached", mark.target);
        }
        Ok(())
    }

    async fn progress(&self, target: StopMarkTarget) -> Result<Progress> {
        let files = match target {
            StopMarkTarget::Download(id) => match self.database.get_download(id).await? {
                Some(file) => vec![file],
                None => return Ok(Progress::Gone),
            },
            StopMarkTarget::Package(id) => {
                if self.database.get_package(id).await?.is_none() {
                    return Ok(Progress::Gone);
                }
                self.database.downloads_for_package(id).await?
            }
        };
        Ok(if files.iter().all(|file| done(file.state)) {
            Progress::Done
        } else {
            Progress::Ahead
        })
    }

    /// `queue.stop_mark`, live only: the mark itself is the row, and a replay after a restart
    /// would report a moment that already passed. Ids only, never a name.
    fn announce_stop_mark(&self, action: &str, target: StopMarkTarget) {
        let (download_id, package_id) = match target {
            StopMarkTarget::Download(id) => (Some(id.to_string()), None),
            StopMarkTarget::Package(id) => (None, Some(id.to_string())),
        };
        self.database.broadcast(EventEnvelope::new(
            EventKind::QueueStopMark,
            serde_json::json!({
                "action": action,
                "download_id": download_id,
                "package_id": package_id,
            }),
        ));
    }
}

#[cfg(test)]
mod tests {
    use rd_core::DownloadState;

    use super::done;

    #[test]
    fn only_a_file_with_nothing_left_to_do_is_done() {
        for state in [
            DownloadState::Completed,
            DownloadState::Failed,
            DownloadState::Cancelled,
            DownloadState::Skipped,
            DownloadState::Seeding,
        ] {
            assert!(done(state), "{state:?}");
        }
        for state in [
            DownloadState::Queued,
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Paused,
            DownloadState::RetryWait,
            DownloadState::Verifying,
            DownloadState::Repairing,
            DownloadState::Extracting,
            DownloadState::Blocked,
        ] {
            assert!(!done(state), "{state:?}");
        }
    }
}
