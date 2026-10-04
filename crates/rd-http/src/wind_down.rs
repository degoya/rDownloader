//! How the workers of one transfer are stopped once one of them has ended the attempt.

use std::time::Duration;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// How long the other workers of a transfer get to write their checkpoint once one of them has
/// ended the attempt; whatever still runs after it is aborted.
const WIND_DOWN: Duration = Duration::from_secs(5);

/// Stops the remaining workers of one transfer at their next checkpoint boundary.
///
/// They used to be aborted mid-write the moment the first one paused or failed, and up to a
/// checkpoint's worth of bytes per chunk was fetched again on every pause (audit 1.9.1, TR-09).
/// A cancelled worker writes its checkpoint and returns, so waiting is short; the timeout only
/// covers one that does not.
pub(crate) async fn wind_down<T: 'static>(tasks: &mut JoinSet<T>, workers: &CancellationToken) {
    workers.cancel();
    let drained = tokio::time::timeout(WIND_DOWN, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tasks.abort_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    use tokio::task::JoinSet;
    use tokio_util::sync::CancellationToken;

    use super::wind_down;

    /// TR-09: the siblings finish their checkpoint, and the caller's token stays its own.
    #[tokio::test]
    async fn siblings_write_their_checkpoint_and_the_callers_token_is_left_alone() {
        let caller = CancellationToken::new();
        let workers = caller.child_token();
        let checkpointed = Arc::new(AtomicUsize::new(0));
        let mut tasks = JoinSet::new();
        for _ in 0..3 {
            let token = workers.clone();
            let checkpointed = Arc::clone(&checkpointed);
            tasks.spawn(async move {
                token.cancelled().await;
                // Stands in for the flush a cancelled worker does before it returns.
                tokio::time::sleep(Duration::from_millis(20)).await;
                checkpointed.fetch_add(1, Ordering::AcqRel);
            });
        }

        wind_down(&mut tasks, &workers).await;

        assert_eq!(checkpointed.load(Ordering::Acquire), 3);
        assert!(tasks.is_empty());
        assert!(!caller.is_cancelled());
    }
}
