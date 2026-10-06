//! Pausing the whole queue until a set time (RD-190-20).
//!
//! The same pause "pause all" has always been — every file that is waiting or moving is paused
//! one by one — with an end attached. Two things come with the end. The files this pause
//! stopped are recorded, so its end resumes exactly those and leaves a file somebody had paused
//! before alone. And the queue is held for as long as it lasts, through the same hold battery
//! and reconnects use, so a link added in the meantime waits instead of starting under a pause.
//!
//! The record is written before any file is touched, and it carries the end, so a restart
//! finds it whichever step the previous run stopped at: before the end the hold is back at
//! once, and an end that passed while the service was down resumes the files on the first
//! tick. A stop between the record and the files leaves them queued, not paused; the restored
//! hold keeps them back until the end all the same (`scheduler.after_queue_pause_recorded`).

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{DownloadId, DownloadState};
use serde::{Deserialize, Serialize};

use crate::{HoldSource, SchedulerHandle};

/// The pause in force, if any; JSON `null` once it has ended.
const QUEUE_PAUSE_KEY: &str = "queue.timed_pause";

/// The reason the queue's hold carries while a timed pause lasts.
const HOLD_REASON: &str = "queue_paused";

/// A pause of the whole queue with the time it ends.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueuePause {
    pub until: DateTime<Utc>,
    /// The files this pause stopped; its end resumes those still paused, and no others.
    pub files: Vec<DownloadId>,
}

/// The states a pause acts on — the web interface's `PAUSABLE_STATES`, waiting and moving.
/// Public for the capture agent's "pause all" (RD-1100-06), which stops the same files.
pub const fn pausable(state: DownloadState) -> bool {
    state.is_queued_or_working()
}

impl SchedulerHandle {
    /// Pauses every waiting and moving file until `until`, then lets them go again.
    ///
    /// A pause that is already in force is moved to the new end and keeps the files it holds,
    /// so pausing "for one more hour" never forgets what the first pause stopped.
    pub async fn pause_queue_until(&self, until: DateTime<Utc>) -> Result<QueuePause> {
        let mut current = self.queue_pause.lock().await;
        let mut files = current
            .as_ref()
            .map(|pause| pause.files.clone())
            .unwrap_or_default();
        let stopping: Vec<DownloadId> = self
            .database
            .list_downloads()
            .await?
            .into_iter()
            .filter(|file| pausable(file.state))
            .map(|file| file.id)
            .collect();
        for id in &stopping {
            if !files.contains(id) {
                files.push(*id);
            }
        }
        let pause = QueuePause { until, files };
        self.store_queue_pause(Some(&pause)).await?;
        *current = Some(pause.clone());
        self.network_hold
            .set(HoldSource::QueuePause, Some(HOLD_REASON))
            .await;
        rd_core::failpoint!("scheduler.after_queue_pause_recorded", || anyhow::anyhow!(
            "crash point scheduler.after_queue_pause_recorded"
        ));
        for id in stopping {
            // A file that finished or was removed a moment ago has nothing left to pause.
            if let Err(error) = self.pause(id).await {
                tracing::debug!(%error, download = %id, "a file could not be paused with the queue");
            }
        }
        Ok(pause)
    }

    /// Ends the pause now: the hold goes, and the files it stopped that are still paused are
    /// queued again. Answers how many were; `0` when no pause was in force.
    pub async fn resume_queue(&self) -> Result<usize> {
        let mut current = self.queue_pause.lock().await;
        let Some(pause) = current.clone() else {
            return Ok(0);
        };
        self.network_hold.set(HoldSource::QueuePause, None).await;
        let mut resumed = 0;
        for id in &pause.files {
            let still_paused = self
                .database
                .get_download(*id)
                .await?
                .is_some_and(|file| file.state == DownloadState::Paused);
            if !still_paused {
                continue;
            }
            match self.resume(*id).await {
                Ok(()) => resumed += 1,
                Err(error) => {
                    tracing::warn!(%error, download = %id, "a file could not be resumed with the queue");
                }
            }
        }
        // Forgotten last: a stop before this line finds the record again and resumes the rest.
        self.store_queue_pause(None).await?;
        *current = None;
        Ok(resumed)
    }

    /// The pause in force, if any.
    pub async fn queue_pause(&self) -> Option<QueuePause> {
        self.queue_pause.lock().await.clone()
    }

    /// Brings back a pause the previous run left, before the first dispatch, so nothing it
    /// held starts in the gap. One whose end has passed is ended by the first tick.
    pub(crate) async fn restore_queue_pause(&self) -> Result<()> {
        let Some(stored) = self.database.get_setting(QUEUE_PAUSE_KEY).await? else {
            return Ok(());
        };
        if stored.is_null() {
            return Ok(());
        }
        let pause = match serde_json::from_value::<QueuePause>(stored) {
            Ok(pause) => pause,
            Err(error) => {
                tracing::warn!(%error, "the stored queue pause was unreadable and is ignored");
                return Ok(());
            }
        };
        self.network_hold
            .set(HoldSource::QueuePause, Some(HOLD_REASON))
            .await;
        *self.queue_pause.lock().await = Some(pause);
        Ok(())
    }

    /// One supervision step: ends the pause once its time has come.
    pub(crate) async fn supervise_queue_pause(&self) -> Result<()> {
        let due = self
            .queue_pause
            .lock()
            .await
            .as_ref()
            .is_some_and(|pause| pause.until <= Utc::now());
        if due {
            let resumed = self.resume_queue().await?;
            tracing::info!(resumed, "the timed queue pause ended");
        }
        Ok(())
    }

    async fn store_queue_pause(&self, pause: Option<&QueuePause>) -> Result<()> {
        self.database
            .set_setting(QUEUE_PAUSE_KEY.to_owned(), serde_json::to_value(pause)?)
            .await
    }
}
