//! The event side: which events become notifications, and for whom.

use super::*;

impl NotificationService {
    /// Turns matching bus events into queued deliveries.
    pub(super) async fn watch_events(self) {
        let mut events = self.inner.database.subscribe();
        loop {
            let event = tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                event = events.recv() => match event {
                    Ok(event) => event,
                    // Not silent any more (audit 1.9.1, INTAKE-04): the skipped events are
                    // notifications nobody gets, and the log is where that shows.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "event bus lagged; notifications for the skipped events were not queued");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            };
            if let Err(error) = self.handle_event(&event).await {
                tracing::warn!(%error, "notification event could not be queued");
            }
        }
    }

    pub(super) async fn handle_event(&self, event: &rd_core::EventEnvelope) -> anyhow::Result<()> {
        let Some((kind, category_id, title, body)) = self.classify(event).await? else {
            return Ok(());
        };
        for rule in self.inner.database.list_notification_rules().await? {
            if !rule.matches(kind, category_id) {
                continue;
            }
            self.inner
                .database
                .queue_notification_delivery(rd_db::NewDelivery {
                    rule_id: rule.id,
                    target_id: rule.target_id,
                    // Derived from the rule and the event id, so a replay after a crash
                    // lands on the same key and the unique index drops it.
                    idempotency_key: rd_notify::idempotency_key(rule.id, &event.id.to_string()),
                    event: kind,
                    title: title.clone(),
                    body: body.clone(),
                })
                .await?;
        }
        Ok(())
    }

    /// What the malware scan found in a package, as one line, when a finding is what stopped it.
    pub(super) async fn malware_finding(&self, package_id: &str) -> Option<String> {
        let steps = self
            .inner
            .database
            .list_postprocess_steps(package_id)
            .await
            .ok()?;
        let step = steps.into_iter().find(|step| {
            step.kind == rd_core::PostprocessKind::MalwareScan
                && step.state == rd_core::PostprocessState::Failed
        })?;
        Some(
            step.message
                .unwrap_or_else(|| "ClamAV found malware".to_owned()),
        )
    }

    /// Maps a bus event onto a notification event, its category and its text.
    #[allow(clippy::type_complexity)]
    pub(super) async fn classify(
        &self,
        event: &rd_core::EventEnvelope,
    ) -> anyhow::Result<
        Option<(
            NotificationEvent,
            Option<rd_core::CategoryId>,
            String,
            String,
        )>,
    > {
        match event.kind {
            rd_core::EventKind::PackageState => {
                let Some(id) = event.payload.get("package_id").and_then(|v| v.as_str()) else {
                    return Ok(None);
                };
                // An id that does not parse names no row, exactly as it matched none before.
                let Ok(package_id) = id.parse::<rd_core::PackageId>() else {
                    return Ok(None);
                };
                let Some(package) = self.inner.database.get_package(package_id).await? else {
                    return Ok(None);
                };
                let kind = match package.state {
                    PackageState::Completed => NotificationEvent::PackageCompleted,
                    PackageState::Failed => NotificationEvent::PackageFailed,
                    _ => return Ok(None),
                };
                // A package the malware scan stopped says so (RD-190-14): "failed" alone would
                // read like a broken archive, and this is the one failure somebody must not
                // retry without looking first.
                if kind == NotificationEvent::PackageFailed
                    && let Some(finding) = self.malware_finding(&package.id.to_string()).await
                {
                    return Ok(Some((
                        kind,
                        package.category_id,
                        format!("Malware found: {}", package.name),
                        format!("{} ({finding})", package.name),
                    )));
                }
                let title = match kind {
                    NotificationEvent::PackageCompleted => {
                        format!("Package finished: {}", package.name)
                    }
                    _ => format!("Package failed: {}", package.name),
                };
                Ok(Some((
                    kind,
                    package.category_id,
                    title,
                    format!("{} ({})", package.name, package.state),
                )))
            }
            rd_core::EventKind::StorageCapacity => Ok(Some((
                NotificationEvent::StorageBlocked,
                None,
                "Storage root out of space".to_owned(),
                "A storage root fell below its free-space threshold and takes no new work."
                    .to_owned(),
            ))),
            rd_core::EventKind::CaptchaChanged => Ok(Some((
                NotificationEvent::CaptchaWaiting,
                None,
                "A captcha is waiting".to_owned(),
                "A download is waiting for a captcha to be answered.".to_owned(),
            ))),
            rd_core::EventKind::PowerChanged => Ok(Some((
                NotificationEvent::PowerPending,
                None,
                "Power action pending".to_owned(),
                "The queue is done and a power action is counting down.".to_owned(),
            ))),
            rd_core::EventKind::BandwidthChanged => Ok(budget_exhausted(&event.payload)),
            rd_core::EventKind::UsenetChanged => match usenet_quota_reached(&event.payload) {
                // One `usenet.changed` arm for both moments (RD-1100-05 quota, RD-1100-02 hopeless).
                Some(quota) => Ok(Some(quota)),
                None => self.usenet_job_hopeless(&event.payload).await,
            },
            _ => Ok(None),
        }
    }

    /// A Usenet set given up as beyond repair (RD-1100-02), in its package's category.
    ///
    /// Every other `usenet.changed` - an import arriving, a server edited - is not a moment
    /// anybody is told about.
    #[allow(clippy::type_complexity)]
    async fn usenet_job_hopeless(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<
        Option<(
            NotificationEvent,
            Option<rd_core::CategoryId>,
            String,
            String,
        )>,
    > {
        if payload.get("state").and_then(|value| value.as_str()) != Some("hopeless") {
            return Ok(None);
        }
        let package = match payload
            .get("package_id")
            .and_then(|value| value.as_str())
            .and_then(|id| id.parse::<rd_core::PackageId>().ok())
        {
            Some(id) => self.inner.database.get_package(id).await?,
            None => None,
        };
        let name = package.as_ref().map_or_else(
            || "A Usenet download".to_owned(),
            |package| package.name.clone(),
        );
        let count = |key: &str| {
            payload
                .get(key)
                .and_then(|value| value.as_str())
                .unwrap_or("?")
                .to_owned()
        };
        Ok(Some((
            NotificationEvent::UsenetJobHopeless,
            package.and_then(|package| package.category_id),
            format!("Usenet download beyond repair: {name}"),
            format!(
                "{name}: {} PAR2 blocks are missing and at most {} can repair them; the rest was not downloaded.",
                count("missing_blocks"),
                count("available_blocks")
            ),
        )))
    }
}
