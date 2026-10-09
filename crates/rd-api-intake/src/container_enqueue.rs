//! Queueing a container's packages once their links are checked (RD-1210-01).
//!
//! An imported link file may go straight on into the download list. The links still take the
//! ordinary road — the blocklist, the online check, the routing rules — and a package is only
//! queued once its check has finished, through the same enqueue a click in the LinkGrabber
//! runs: a package that is still being checked refuses the enqueue, so waiting is the only
//! honest order. Nothing is kept across a restart: a package whose check outlives the service,
//! or this wait, stays in the LinkGrabber, where it would have been without the option.

use std::{collections::HashSet, time::Duration};

use rd_api_core::link_check_service::CompletedBatches;
use rd_core::{BatchId, CollectorPackage, CollectorPackageId};
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;

/// How long the import waits for its check before leaving the packages to the LinkGrabber.
const WAIT_LIMIT: Duration = Duration::from_secs(30 * 60);

/// Queues `packages` once the check of every batch they belong to has finished.
///
/// `completed` must be followed before the import starts its check, or a fast check is over
/// before anybody listens.
pub(crate) fn after_check(
    state: AppState,
    completed: CompletedBatches,
    packages: &[CollectorPackage],
) {
    let batches: HashSet<BatchId> = packages.iter().map(|package| package.batch_id).collect();
    let packages: Vec<CollectorPackageId> = packages.iter().map(|package| package.id).collect();
    tokio::spawn(async move {
        match tokio::time::timeout(WAIT_LIMIT, checked(completed, batches)).await {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                tracing::warn!(
                    packages = packages.len(),
                    "an imported file's check did not finish in time; its packages stay in the LinkGrabber"
                );
                return;
            }
        }
        for package_id in packages {
            if let Err(error) =
                crate::collector_enqueue::enqueue_package(&state, package_id, false, None).await
            {
                // One package whose links all went offline must not keep the others back.
                tracing::warn!(
                    %package_id,
                    error = %error.message(),
                    "an imported package could not be queued"
                );
            }
        }
    });
}

/// Whether every batch's check was announced; `false` when the announcements were lost.
async fn checked(mut completed: CompletedBatches, mut open: HashSet<BatchId>) -> bool {
    while !open.is_empty() {
        match completed.recv().await {
            Ok(batch) => {
                open.remove(&batch);
            }
            // Ids no longer kept may have been ours; guessing would queue a package mid-check.
            Err(RecvError::Lagged(_) | RecvError::Closed) => return false,
        }
    }
    true
}
