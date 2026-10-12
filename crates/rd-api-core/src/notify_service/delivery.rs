//! The delivery side: the outbox sweep and one attempt at one target.

use super::*;

impl NotificationService {
    /// Works the delivery queue.
    pub(super) async fn deliver_loop(self) {
        let mut ticker = tokio::time::interval(SWEEP);
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                _ = ticker.tick() => {
                    if let Err(error) = self.sweep().await {
                        tracing::warn!(%error, "notification delivery sweep failed");
                    }
                }
            }
        }
    }

    pub(super) async fn sweep(&self) -> anyhow::Result<()> {
        let now = chrono::Utc::now();
        // Quiet hours group deliveries rather than dropping them: the queue keeps filling, and
        // what it held is one summary per target when the window ends (RD-1260-04).
        if self.inner.power.defers_notifications(now).await {
            return Ok(());
        }
        let due = self.inner.database.due_notification_deliveries(now).await?;
        if due.is_empty() {
            return Ok(());
        }
        let targets: HashMap<_, _> = self
            .inner
            .database
            .list_notification_targets()
            .await?
            .into_iter()
            .map(|target| (target.id, target))
            .collect();
        // Per target, in the order they came due.
        let mut per_target: Vec<(NotificationTargetId, Vec<Delivery>)> = Vec::new();
        for delivery in due {
            match per_target
                .iter_mut()
                .find(|(target, _)| *target == delivery.target_id)
            {
                Some((_, deliveries)) => deliveries.push(delivery),
                None => per_target.push((delivery.target_id, vec![delivery])),
            }
        }
        for (target_id, deliveries) in per_target {
            let Some(target) = targets.get(&target_id) else {
                continue;
            };
            if !target.enabled {
                continue;
            }
            // One attempt per target at a time: a target that times out must not stall the
            // sweep for every other one.
            if !self.inner.in_flight.lock().await.insert(target.id) {
                continue;
            }
            let batch = self.next_batch(deliveries).await;
            let service = self.clone();
            let target = target.clone();
            tokio::spawn(async move {
                service.attempt(batch, target).await;
            });
        }
        Ok(())
    }

    /// What one attempt at a target carries: everything quiet hours held back for it, as one
    /// summary, or else the oldest due delivery. A delivery counts as held when it was queued
    /// while notifications waited.
    async fn next_batch(&self, deliveries: Vec<Delivery>) -> Vec<Delivery> {
        let mut held = Vec::new();
        let mut first = None;
        for delivery in deliveries {
            if self
                .inner
                .power
                .defers_notifications(delivery.created_at)
                .await
            {
                held.push(delivery);
            } else if first.is_none() {
                first = Some(delivery);
            }
        }
        if held.is_empty() {
            first.into_iter().collect()
        } else {
            held
        }
    }

    /// Delivers `batch` -- one delivery, or a quiet-hours summary of several -- and records the
    /// outcome on each of them.
    pub(super) async fn attempt(&self, batch: Vec<Delivery>, target: NotificationTarget) {
        let Some(message) = rd_notify::quiet_summary(&batch) else {
            self.inner.in_flight.lock().await.remove(&target.id);
            return;
        };
        let config = target_config(&target);
        // A summary names no single package, so a media server refreshes its whole library.
        let package_path = match batch.as_slice() {
            [only] => only.package_path.clone(),
            _ => None,
        };
        let outcome = if target.kind == rd_notify::TargetKind::Plugin {
            self.deliver_through_plugin(&target, &config, &message, package_path.as_deref())
                .await
        } else if target.kind == rd_notify::TargetKind::WebPush {
            self.deliver_web_push(&message, false).await
        } else {
            match self.resolve_secret(&target).await {
                Ok(secret) => {
                    let vendor = vendor_directory(&self.inner.database).await;
                    rd_notify::send(
                        &super::webhook_reach(&target),
                        &target,
                        &config,
                        &message,
                        secret.as_ref(),
                        vendor.as_deref(),
                    )
                    .await
                }
                Err(unreadable) => unreadable,
            }
        };
        self.inner.in_flight.lock().await.remove(&target.id);

        for delivery in batch {
            let attempt = delivery.attempt.saturating_add(1);
            let (state, next) = settle(&outcome, attempt, chrono::Utc::now());
            if state == DeliveryState::Failed {
                // Repeated failure is worth surfacing: a target nobody notices is broken is
                // worse than no target at all.
                tracing::warn!(
                    target = %target.name,
                    attempts = attempt,
                    status = ?outcome.status,
                    "notification target gave up"
                );
            }
            if let Err(error) = self
                .inner
                .database
                .record_notification_attempt(
                    delivery.id,
                    state,
                    attempt,
                    next,
                    outcome.status,
                    outcome.excerpt.clone(),
                )
                .await
            {
                tracing::warn!(%error, "delivery attempt could not be recorded");
            }
        }
    }
}
