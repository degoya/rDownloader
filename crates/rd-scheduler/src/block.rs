//! Why a download is blocked, and how a running one is stopped into that state.
//!
//! `Blocked` is one queue state for several unrelated causes, and that is deliberate: to the
//! operator they all read "this is not going to start on its own". To the release paths they
//! are not interchangeable at all. Freeing disk space has to restart exactly the transfers the
//! full disk stopped; restarting one whose validators changed mid-flight overwrites confirmed
//! bytes with a different file's content, which is the case the block exists to prevent, and
//! restarting one of a kind the operator switched off ignores a decision they made on purpose.
//!
//! So the cause is recorded on the row and every release names the cause it undoes.

use anyhow::{Context, Result};
use rd_core::{DownloadId, DownloadState};

use crate::{SchedulerHandle, StopReason};

/// Why a download sits in [`DownloadState::Blocked`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockReason {
    /// The storage root the package writes to fell below its free-space threshold.
    Capacity,
    /// The file changed on the host while a partial copy was already on disk.
    ValidatorsChanged,
    /// A resume needs range requests and the host refused them.
    RangesRefused,
    /// The operator switched this download kind off.
    KindDisabled,
}

impl BlockReason {
    /// The value persisted in `downloads.block_reason`.
    ///
    /// Stable on purpose: it is read back after a restart, so renaming one of these silently
    /// orphans every row an earlier build wrote and the matching release stops finding them.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capacity => "capacity",
            Self::ValidatorsChanged => "validators-changed",
            Self::RangesRefused => "ranges-refused",
            Self::KindDisabled => "kind-disabled",
        }
    }
}

impl SchedulerHandle {
    /// Stops a running transfer and parks it in `Blocked`, with the cause recorded.
    ///
    /// The capacity supervisor used to call [`Self::pause`] here. That parks the transfer in
    /// `Paused`, which no release path ever leaves again — so the transfers a full disk
    /// stopped were never the ones that freeing space restarted, and they sat there for good.
    pub(crate) async fn block(&self, id: DownloadId, reason: BlockReason) -> Result<()> {
        let token = {
            let mut active = self.active.lock().await;
            active.reasons.insert(id, StopReason::Blocked(reason));
            active.tokens.get(&id).cloned()
        };
        // A running file writes its own state when it unwinds; writing it here as well would
        // race the transition the worker is on its way to making.
        if let Some(token) = token {
            token.cancel();
            return Ok(());
        }
        let current = self
            .database
            .get_download(id)
            .await?
            .context("download not found")?;
        if matches!(
            current.state,
            DownloadState::Downloading | DownloadState::Resolving
        ) {
            self.database.block_download(id, reason.as_str()).await?;
        }
        Ok(())
    }
}
