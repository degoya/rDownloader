//! Database facade for pausing and resuming many queue rows in one writer transaction
//! (RD-1250-08).

use anyhow::Result;
use rd_core::DownloadId;

use crate::{Database, commands::DownloadsCommand, writer};

impl Database {
    /// Pauses every waiting row of `ids` — `queued` or `retry_wait` — in one transaction, with
    /// one `download.state` event for the rows that moved.
    ///
    /// One answer per id, in order: `true` for a row that moved, `false` for a row in any other
    /// state, left as it was as the single pause leaves it, and a refusal for a row that is
    /// gone.
    ///
    /// # Errors
    ///
    /// When the transaction itself fails; then no row of the batch moved.
    pub async fn pause_downloads(&self, ids: Vec<DownloadId>) -> Result<Vec<Result<bool>>> {
        writer::request(&self.writer, |reply| DownloadsCommand::PauseDownloads {
            ids,
            reply,
        })
        .await
    }

    /// Queues the rows of `ids` again in one transaction, with one `download.state` event for
    /// the rows that moved: a failed row with a fresh retry budget, as
    /// [`Self::retry_failed_download`] gives it, any other by the ordinary transition, refused
    /// on its own where the state does not allow it.
    ///
    /// `false` names a waiting mirror, left as it was: its resume stands its siblings down
    /// first, which is the scheduler's single resume to do.
    ///
    /// # Errors
    ///
    /// When the transaction itself fails; then no row of the batch moved.
    pub async fn resume_downloads(&self, ids: Vec<DownloadId>) -> Result<Vec<Result<bool>>> {
        writer::request(&self.writer, |reply| DownloadsCommand::ResumeDownloads {
            ids,
            reply,
        })
        .await
    }
}
