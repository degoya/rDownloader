use anyhow::{Context, Result, bail};
use rd_core::{DownloadId, DownloadState};
use rd_db::StoreError;

use crate::{SchedulerHandle, StopReason};

#[path = "control_relocate.rs"]
mod relocate;
#[path = "control_removal.rs"]
mod removal;

#[cfg(test)]
use removal::discard_scratch_files;
pub(crate) use removal::remove_part_file;

/// A reset refused because the Usenet file's package let go of its NZB after it completed.
///
/// With the NZB gone there are no articles left to fetch, so a queued row could only fail; the
/// refusal comes before the reset touches the file on disk (re-audit 1.9.1, RA-DB-01). A type
/// of its own so the interface can say why instead of "pause it first".
#[derive(Debug)]
pub struct NzbDropped;

impl std::fmt::Display for NzbDropped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a usenet download whose NZB was dropped cannot be fetched again")
    }
}

impl std::error::Error for NzbDropped {}

impl SchedulerHandle {
    /// Requests a safe pause at the next checkpoint boundary.
    pub async fn pause(&self, id: DownloadId) -> Result<()> {
        let token = {
            let mut active = self.active.lock().await;
            active.reasons.insert(id, StopReason::Paused);
            active.tokens.get(&id).cloned()
        };
        if let Some(token) = token {
            token.cancel();
            return Ok(());
        }
        let written = async {
            let current = self
                .database
                .get_download(id)
                .await?
                .context(StoreError::not_found("download not found"))?;
            if matches!(
                current.state,
                DownloadState::Queued | DownloadState::RetryWait
            ) {
                self.database
                    .transition_download(id, DownloadState::Paused)
                    .await?;
            }
            anyhow::Ok(())
        }
        .await;
        self.release_stop_guard(id).await;
        written
    }

    /// Lets go of the stop reason recorded for a file no worker runs, once its row is written.
    ///
    /// The reason is recorded first all the same, because the dispatcher skips every id that
    /// has one: that keeps it from starting the file between the check and the write. Left in
    /// place afterwards it outlived its purpose - only a worker's end, a resume or a reset took
    /// it out, so a paused mirror that was woken to `Queued` later, or a row set back some
    /// other way, was skipped for good until the process restarted.
    pub(crate) async fn release_stop_guard(&self, id: DownloadId) {
        let mut active = self.active.lock().await;
        if !active.tokens.contains_key(&id) {
            active.reasons.remove(&id);
        }
    }

    /// Runs `work` on a file no worker runs while a hold keeps the dispatcher off it, and lets
    /// the hold go afterwards whatever `work` answered.
    ///
    /// Refused with `refusal` when a worker holds the file. A queued or retry-waiting row is
    /// startable the whole time: a reset that let go of the reason before deleting the `.part`
    /// let the dispatcher start it in between, the worker resumed at the row's checkpoint in a
    /// new empty file, and the payload began with zeros (audit 1.9.1, TR-04). The hold is a
    /// count of its own rather than a stop reason, because a resume, the end of a pause or
    /// cancel, or a second hold on the same row took the reason out in the middle of the work
    /// (re-audit 1.9.1, RA-TR-02). A stop reason left from the pause or cancel that made the
    /// work legal still goes at the end, as it did.
    pub(crate) async fn while_held<T>(
        &self,
        id: DownloadId,
        refusal: &'static str,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        {
            let mut active = self.active.lock().await;
            if active.tokens.contains_key(&id) {
                bail!(StoreError::wrong_state(refusal));
            }
            *active.held.entry(id).or_default() += 1;
        }
        let result = work.await;
        self.active.lock().await.release_hold(&id);
        self.release_stop_guard(id).await;
        result
    }

    /// Moves a paused, failed, blocked or cancelled job back to the queue.
    ///
    /// Refused while a reset or removal holds the row: queued in the middle of it, the row was
    /// written back by the reset anyway, or deleted under a resume that said it was queued.
    pub async fn resume(&self, id: DownloadId) -> Result<()> {
        {
            let mut active = self.active.lock().await;
            if active.held.contains_key(&id) {
                bail!(StoreError::wrong_state(
                    "the download is being reset or removed right now"
                ));
            }
            active.reasons.remove(&id);
        }
        // Starting a waiting mirror by hand is a decision about which link to use, so its
        // siblings stand down for it. Without this the dispatcher would put it straight back:
        // it picks the group's member by the same rule that chose the current one.
        self.stand_down_siblings_of(id).await?;
        // A failed download starts again with a fresh retry budget, as a round of the automatic
        // retry does but uncounted (RD-191-12): with the spent attempts kept, its next transient
        // failure ended it at once. Anything else keeps its counters; a row that left `failed`
        // meanwhile takes the ordinary transition.
        let failed = self
            .database
            .get_download(id)
            .await?
            .is_some_and(|file| file.state == DownloadState::Failed);
        if failed && self.database.retry_failed_download(id).await?.is_some() {
            return Ok(());
        }
        self.database
            .transition_download(id, DownloadState::Queued)
            .await?;
        Ok(())
    }

    /// Stands the other members of `id`'s mirror group down, so this one gets the turn.
    ///
    /// Only members that have not started: one that is already downloading keeps what it has
    /// done, and the dispatcher settles the pair on its next pass rather than throwing work
    /// away here.
    async fn stand_down_siblings_of(&self, id: DownloadId) -> Result<()> {
        let Some(file) = self.database.get_download(id).await? else {
            return Ok(());
        };
        if file.mirror_group.is_none() || file.state != DownloadState::Skipped {
            return Ok(());
        }
        let downloads = self.database.downloads_for_package(file.package_id).await?;
        let siblings = crate::mirrors::siblings(&file, &downloads);
        // Refused rather than silently reverted: the dispatcher would put this one straight
        // back and nothing would say why. Throwing away a transfer that is already under way
        // is not something to do on a resume click either.
        if let Some(active) = siblings
            .iter()
            .find(|sibling| crate::mirrors::has_taken_the_turn(sibling.state))
        {
            bail!(crate::mirrors::MirrorTaken {
                source: active.source.to_string()
            });
        }
        for sibling in siblings {
            if crate::mirrors::is_contending(sibling.state) {
                self.database
                    .transition_download(sibling.id, DownloadState::Skipped)
                    .await?;
            }
        }
        Ok(())
    }

    /// Cancels a queued or active job without deleting partial data.
    pub async fn cancel(&self, id: DownloadId) -> Result<()> {
        let token = {
            let mut active = self.active.lock().await;
            active.reasons.insert(id, StopReason::Cancelled);
            active.tokens.get(&id).cloned()
        };
        if let Some(token) = token {
            token.cancel();
        } else {
            let written = async {
                let current = self
                    .database
                    .get_download(id)
                    .await?
                    .context(StoreError::not_found("download not found"))?;
                // Tagged, so the interface can say why instead of reporting an internal error.
                if !current.state.can_transition_to(DownloadState::Cancelled) {
                    bail!(StoreError::wrong_state(format!(
                        "a download in state {} cannot be cancelled",
                        current.state
                    )));
                }
                self.database
                    .transition_download(id, DownloadState::Cancelled)
                    .await?;
                anyhow::Ok(())
            }
            .await;
            self.release_stop_guard(id).await;
            written?;
        }
        // Cancelling the member that held the group's turn is as final as running out of
        // retries; without this its mirrors wait for a link that is never coming back.
        self.promote_mirror_of(id).await;
        Ok(())
    }

    /// Lets a waiting mirror take over when the member holding the turn steps aside.
    ///
    /// Failure to promote is logged rather than propagated: it must not turn a cancel or a
    /// removal that already happened into an error the caller has to undo.
    async fn promote_mirror_of(&self, id: DownloadId) {
        let Ok(Some(file)) = self.database.get_download(id).await else {
            return;
        };
        if file.mirror_group.is_none() {
            return;
        }
        if let Err(error) = crate::failures::wake_mirror(self, &file).await {
            tracing::warn!(%error, download_id = %id, "no mirror could take over");
        }
    }
}

fn is_active(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Repairing
            | DownloadState::Extracting
    )
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
