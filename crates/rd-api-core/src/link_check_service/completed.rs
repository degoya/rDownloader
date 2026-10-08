//! The announcement of finished batch checks, with the recent ones kept (CORE-01).
//!
//! The same idea as the event bus's follower: a waiter that fell further behind than the live
//! channel holds is told `Lagged` and the channel skips ahead; it then takes the skipped batch
//! ids from the kept ones, with a fresh receiver from the same step, so the subscription
//! auto-queue promotes every batch of a burst instead of leaving them in the LinkGrabber.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
};

use rd_core::BatchId;
use tokio::sync::broadcast::{self, error::RecvError};

/// Capacity of the live channel per waiter before it is reported as lagged.
const LIVE_CAPACITY: usize = 64;

/// How many announcements are kept for a waiter that fell behind.
const KEPT: usize = 1024;

/// The sender side, shared by the check task and every waiter.
pub(super) struct Completed {
    state: Mutex<State>,
}

struct State {
    live: broadcast::Sender<BatchId>,
    /// The newest announcements, oldest first; the last one has sequence number `next - 1`.
    kept: VecDeque<BatchId>,
    /// The sequence number the next announcement gets: how many were ever sent.
    next: u64,
}

impl Completed {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                live: broadcast::channel(LIVE_CAPACITY).0,
                kept: VecDeque::new(),
                next: 0,
            }),
        })
    }

    /// Keeps the batch id and hands it to every waiter, in one step.
    pub(super) fn announce(&self, batch: BatchId) {
        let mut state = self.lock();
        state.kept.push_back(batch);
        if state.kept.len() > KEPT {
            state.kept.pop_front();
        }
        state.next += 1;
        // No waiter is no error: nobody is waiting for the batch.
        let _ = state.live.send(batch);
    }

    pub(super) fn follow(self: &Arc<Self>) -> CompletedBatches {
        let state = self.lock();
        CompletedBatches {
            source: Arc::downgrade(self),
            live: state.live.subscribe(),
            next: state.next,
            replayed: VecDeque::new(),
        }
    }

    /// The kept ids from sequence number `from` on, a receiver for everything announced from
    /// now on, the sequence number it starts at, and how many ids from `from` on were not kept.
    fn since(&self, from: u64) -> (VecDeque<BatchId>, broadcast::Receiver<BatchId>, u64, u64) {
        let state = self.lock();
        let oldest = state.next - state.kept.len() as u64;
        let skip = usize::try_from(from.saturating_sub(oldest)).unwrap_or(usize::MAX);
        (
            state.kept.iter().skip(skip).copied().collect(),
            state.live.subscribe(),
            state.next,
            oldest.saturating_sub(from),
        )
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Every write is a push, a pop or an increment; none can panic halfway.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A waiter for finished batch checks that does not lose one to a burst.
pub struct CompletedBatches {
    /// Weak, so the channel still closes when the check service goes away.
    source: Weak<Completed>,
    live: broadcast::Receiver<BatchId>,
    /// The sequence number of the next id the live receiver hands over.
    next: u64,
    replayed: VecDeque<BatchId>,
}

impl CompletedBatches {
    /// The next finished batch; `Lagged(n)` only when `n` ids were no longer kept, `Closed`
    /// when the check service is gone.
    pub async fn recv(&mut self) -> Result<BatchId, RecvError> {
        loop {
            if let Some(batch) = self.replayed.pop_front() {
                return Ok(batch);
            }
            match self.live.recv().await {
                Ok(batch) => {
                    self.next += 1;
                    return Ok(batch);
                }
                Err(RecvError::Lagged(_)) => {
                    let Some(source) = self.source.upgrade() else {
                        return Err(RecvError::Closed);
                    };
                    let (replayed, live, next, lost) = source.since(self.next);
                    self.replayed = replayed;
                    self.live = live;
                    self.next = next;
                    if lost > 0 {
                        return Err(RecvError::Lagged(lost));
                    }
                }
                Err(closed) => return Err(closed),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Completed, KEPT, LIVE_CAPACITY};
    use rd_core::BatchId;
    use tokio::sync::broadcast::error::RecvError;

    /// More finished batches than the live channel holds, before the waiter reads one: every
    /// one still arrives, in order.
    #[tokio::test]
    async fn a_waiter_behind_a_full_channel_misses_no_batch() {
        let completed = Completed::new();
        let mut waiter = completed.follow();
        let batches: Vec<BatchId> = (0..LIVE_CAPACITY + 20).map(|_| BatchId::new()).collect();
        for batch in &batches {
            completed.announce(*batch);
        }

        for batch in &batches {
            assert_eq!(
                waiter.recv().await.expect("every batch of the burst"),
                *batch
            );
        }
        let after = BatchId::new();
        completed.announce(after);
        assert_eq!(
            waiter.recv().await.expect("the batch after the burst"),
            after
        );
    }

    /// Only what was no longer kept is reported, with its number; the kept rest follows.
    #[tokio::test]
    async fn a_waiter_hears_how_many_batches_were_not_kept() {
        let completed = Completed::new();
        let mut waiter = completed.follow();
        let batches: Vec<BatchId> = (0..KEPT + 5).map(|_| BatchId::new()).collect();
        for batch in &batches {
            completed.announce(*batch);
        }

        assert!(matches!(waiter.recv().await, Err(RecvError::Lagged(5))));
        for batch in &batches[5..] {
            assert_eq!(waiter.recv().await.expect("the kept rest"), *batch);
        }
    }
}
