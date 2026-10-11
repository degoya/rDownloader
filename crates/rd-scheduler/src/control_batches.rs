//! Pausing and resuming many files at once (RD-1250-08): the stop reasons and holds under one
//! lock, the running files stopped together, the rest in one writer transaction.

use anyhow::Result;
use rd_core::DownloadId;
use rd_db::StoreError;

use crate::{SchedulerHandle, StopReason};

impl SchedulerHandle {
    /// [`Self::pause`] for many files: one answer per id, in order.
    ///
    /// Every file gets its stop reason under one lock, so the dispatcher starts none of them in
    /// between; the running ones have their tokens cancelled together and write their pause
    /// themselves at the next checkpoint, as with the single pause. The waiting ones are paused
    /// in one transaction, and their stop reasons go again afterwards. A file in any other
    /// state is left as it is and answered as done, as the single pause answers it.
    pub async fn pause_many(&self, ids: &[DownloadId]) -> Vec<Result<()>> {
        let mut idle = Vec::new();
        let mut positions = Vec::new();
        let running: Vec<_> = {
            let mut active = self.active.lock().await;
            let mut running = Vec::new();
            for (index, &id) in ids.iter().enumerate() {
                active.reasons.insert(id, StopReason::Paused);
                match active.tokens.get(&id) {
                    Some(token) => running.push(token.clone()),
                    None => {
                        idle.push(id);
                        positions.push(index);
                    }
                }
            }
            running
        };
        for token in running {
            token.cancel();
        }
        let mut outcomes: Vec<Result<()>> = ids.iter().map(|_| Ok(())).collect();
        if idle.is_empty() {
            return outcomes;
        }
        let answers = self.database.pause_downloads(idle.clone()).await;
        {
            let mut active = self.active.lock().await;
            for id in &idle {
                if !active.tokens.contains_key(id) {
                    active.reasons.remove(id);
                }
            }
        }
        for (index, answer) in positions
            .into_iter()
            .zip(batch_answers(answers, idle.len()))
        {
            if let Some(slot) = outcomes.get_mut(index) {
                *slot = answer.map(|_| ());
            }
        }
        outcomes
    }

    /// [`Self::resume`] for many files: one answer per id, in order.
    ///
    /// The holds are looked at and the stop reasons taken out under one lock, a file a reset or
    /// removal holds is refused as there, and the rest are queued again in one transaction. A
    /// waiting mirror goes the single resume afterwards, file by file: starting it is a choice
    /// between links, which stands its siblings down first.
    pub async fn resume_many(&self, ids: &[DownloadId]) -> Vec<Result<()>> {
        let mut outcomes: Vec<Result<()>> = Vec::with_capacity(ids.len());
        let mut batch = Vec::new();
        let mut positions = Vec::new();
        {
            let mut active = self.active.lock().await;
            for (index, &id) in ids.iter().enumerate() {
                if active.held.contains_key(&id) {
                    outcomes.push(Err(anyhow::anyhow!(StoreError::wrong_state(
                        "the download is being reset or removed right now"
                    ))));
                    continue;
                }
                active.reasons.remove(&id);
                batch.push(id);
                positions.push(index);
                outcomes.push(Ok(()));
            }
        }
        if batch.is_empty() {
            return outcomes;
        }
        let answers = self.database.resume_downloads(batch.clone()).await;
        let answers = batch_answers(answers, batch.len());
        for ((index, id), answer) in positions.into_iter().zip(batch).zip(answers) {
            let answer = match answer {
                Ok(true) => Ok(()),
                Ok(false) => self.resume(id).await,
                Err(error) => Err(error),
            };
            if let Some(slot) = outcomes.get_mut(index) {
                *slot = answer;
            }
        }
        outcomes
    }
}

/// The answers of one batch transaction, or the transaction's own failure once per row: then no
/// row of the batch moved.
fn batch_answers(answers: Result<Vec<Result<bool>>>, rows: usize) -> Vec<Result<bool>> {
    match answers {
        Ok(answers) => answers,
        Err(error) => {
            let message = format!("{error:#}");
            (0..rows)
                .map(|_| Err(anyhow::anyhow!(message.clone())))
                .collect()
        }
    }
}

#[cfg(test)]
#[path = "control_batches_tests.rs"]
mod tests;
