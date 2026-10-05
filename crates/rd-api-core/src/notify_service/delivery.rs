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
        // Quiet hours group deliveries rather than dropping them: the queue keeps filling
        // and is worked off in one go when the window ends.
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
        for delivery in due {
            let Some(target) = targets.get(&delivery.target_id) else {
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
            let service = self.clone();
            let target = target.clone();
            tokio::spawn(async move {
                service.attempt(delivery, target).await;
            });
        }
        Ok(())
    }

    pub(super) async fn attempt(&self, delivery: Delivery, target: NotificationTarget) {
        let config = target_config(&target);
        let message = Message {
            title: delivery.title.clone(),
            body: delivery.body.clone(),
            event: delivery.event,
            idempotency_key: delivery.idempotency_key.clone(),
            payload: serde_json::json!({
                "event": delivery.event,
                "title": delivery.title,
                "body": delivery.body,
                "idempotency_key": delivery.idempotency_key,
            }),
        };
        let outcome = if target.kind == rd_notify::TargetKind::Plugin {
            self.deliver_through_plugin(&target, &config, &message)
                .await
        } else {
            let secret = self.resolve_secret(&target).await;
            let vendor = vendor_directory(&self.inner.database).await;
            rd_notify::send(
                &self.inner.http,
                &target,
                &config,
                &message,
                secret.as_ref(),
                vendor.as_deref(),
            )
            .await
        };
        self.inner.in_flight.lock().await.remove(&target.id);

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
                outcome.excerpt,
            )
            .await
        {
            tracing::warn!(%error, "delivery attempt could not be recorded");
        }
    }
}
