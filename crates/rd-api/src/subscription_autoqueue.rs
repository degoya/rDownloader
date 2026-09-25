//! Completes the AutoQueue path of a subscription (RD-091).
//!
//! A subscription in `AutoQueue` mode marks its accepted items `Queued` and hands their links
//! to the ordinary LinkGrabber intake — deliberately the same path a pasted link takes. What
//! was missing is the step after that: nothing ever promoted the checked candidates into the
//! download queue, so an auto-queue subscription reported "queued" while its links sat in the
//! LinkGrabber for good.
//!
//! This is driven from here rather than from `SubscriptionService` because enqueuing needs the
//! full [`AppState`], which is assembled after the poller exists.
//!
//! Restart behaviour: a batch whose check was still running when the service stopped is not
//! promoted afterwards. Its links stay visible in the LinkGrabber and can be added by hand —
//! the same place they would have been had the check never finished.

use crate::AppState;

/// Watches finished link checks and enqueues the ones an auto-queue subscription submitted.
///
/// Ends when the check service's sender is dropped, which happens with the application state.
pub fn start(state: AppState) {
    let mut completed = state.link_check.subscribe_completed();
    tokio::spawn(async move {
        loop {
            let batch_id = match completed.recv().await {
                Ok(batch_id) => batch_id,
                // Lagged: batches were checked faster than this task read them. The dropped
                // ones stay in the LinkGrabber rather than being guessed at.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(
                        missed,
                        "auto-queue watcher fell behind; those batches stay in the LinkGrabber"
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            if let Err(error) = promote(&state, batch_id).await {
                tracing::warn!(%batch_id, %error, "auto-queue enqueue failed");
            }
        }
    });
}

/// Enqueues every package of `batch_id`, if it came from an auto-queue subscription.
async fn promote(state: &AppState, batch_id: rd_core::BatchId) -> anyhow::Result<()> {
    let Some(batch) = state
        .database
        .list_collector_batches()
        .await?
        .into_iter()
        .find(|batch| batch.id == batch_id)
    else {
        return Ok(());
    };
    if batch.source != rd_core::IngressSource::Subscription {
        return Ok(());
    }
    // The batch records which subscription submitted it by name; only that subscription's
    // current mode decides, so switching a subscription back to review takes effect at once.
    let Some(label) = batch.source_label.as_deref() else {
        return Ok(());
    };
    let auto_queue = state
        .database
        .list_subscriptions()
        .await?
        .into_iter()
        .any(|subscription| {
            subscription.name == label && subscription.mode == rd_core::SubscriptionMode::AutoQueue
        });
    if !auto_queue {
        return Ok(());
    }

    let packages: Vec<rd_core::CollectorPackageId> = {
        let mut seen = Vec::new();
        for candidate in state.database.list_candidates().await? {
            if candidate.batch_id != batch_id {
                continue;
            }
            if let Some(package_id) = candidate.package_id
                && !seen.contains(&package_id)
            {
                seen.push(package_id);
            }
        }
        seen
    };
    for package_id in packages {
        match crate::collector_enqueue::enqueue_package(state, package_id, false, None).await {
            Ok(outcome) => tracing::info!(
                subscription = %label,
                package = %outcome.package.name,
                "auto-queue subscription enqueued a package"
            ),
            // One unusable package must not stop the rest of the batch: a release whose links
            // all went offline between the check and here is ordinary, not an error worth
            // abandoning the poll for.
            Err(error) => tracing::warn!(
                subscription = %label,
                %package_id,
                error = %error.message(),
                "auto-queue package could not be enqueued"
            ),
        }
    }
    Ok(())
}
