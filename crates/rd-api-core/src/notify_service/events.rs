//! The event side: which events become notifications, and for whom.

use super::*;

impl NotificationService {
    /// Turns matching bus events into queued deliveries.
    pub(super) async fn watch_events(self) {
        // A follower takes what a burst pushed past the live channel from the bus's buffer
        // (CORE-01); `Lagged` is left for what fell out of the buffer as well.
        let mut events = self.inner.database.follow();
        // The open bursts of links and download starts close on this tick (RD-1240-17).
        let mut bursts = tokio::time::interval(super::bursts::BURST_TICK);
        bursts.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let event = tokio::select! {
                () = self.inner.shutdown.cancelled() => {
                    // Queued now rather than lost: the delivery queue outlives the restart.
                    self.flush_bursts(true).await;
                    return;
                }
                _ = bursts.tick() => {
                    self.flush_bursts(false).await;
                    continue;
                }
                event = events.recv() => match event {
                    Ok(event) => event,
                    // Not silent any more (audit 1.9.1, INTAKE-04): the lost events are
                    // notifications nobody gets, and the log is where that shows.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "event bus lagged beyond its buffer; notifications for the lost events were not queued");
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
        if let Some(kind) = super::bursts::coalescing_kind(event) {
            return self.fold_into_burst(kind, event).await;
        }
        let Some((kind, category_id, title, body)) = self.classify(event).await? else {
            return Ok(());
        };
        let package_path = if kind == NotificationEvent::PackageCompleted {
            self.package_folder(&event.payload).await?
        } else {
            None
        };
        // A rule's own wording is filled here, once, so a retry sends what the first attempt
        // did (RD-1280-02).
        let values = self.wanted_values(kind, category_id, event).await?;
        self.queue_worded(
            kind,
            category_id,
            &event.id.to_string(),
            (&title, &body),
            package_path,
            values.as_ref(),
        )
        .await
    }

    /// The folder of the package a `package.state` event names (RD-1250-01): what a media
    /// server is asked to scan once the package is finished. Its destination is the folder
    /// post-processing worked in and left the files in.
    async fn package_folder(&self, payload: &serde_json::Value) -> anyhow::Result<Option<String>> {
        let Some(id) = payload
            .get("package_id")
            .and_then(|value| value.as_str())
            .and_then(|id| id.parse::<rd_core::PackageId>().ok())
        else {
            return Ok(None);
        };
        Ok(self
            .inner
            .database
            .get_package(id)
            .await?
            .map(|package| package.destination)
            .filter(|folder| !folder.trim().is_empty()))
    }

    /// Queues one delivery of an event for every rule that wants it. `package_path` is the
    /// finished package's folder, kept with each delivery so a retry names it too.
    pub(super) async fn queue_event(
        &self,
        kind: NotificationEvent,
        category_id: Option<rd_core::CategoryId>,
        event_id: &str,
        title: &str,
        body: &str,
        package_path: Option<String>,
    ) -> anyhow::Result<()> {
        self.queue_worded(
            kind,
            category_id,
            event_id,
            (title, body),
            package_path,
            None,
        )
        .await
    }

    /// [`Self::queue_event`], each rule with a template sending its own wording filled with
    /// `values` (RD-1280-02); without values, every rule sends the built-in one.
    pub(super) async fn queue_worded(
        &self,
        kind: NotificationEvent,
        category_id: Option<rd_core::CategoryId>,
        event_id: &str,
        (title, body): (&str, &str),
        package_path: Option<String>,
        values: Option<&rd_notify::Values>,
    ) -> anyhow::Result<()> {
        for rule in self.inner.database.list_notification_rules().await? {
            if !rule.matches(kind, category_id) {
                continue;
            }
            let (title, body) = values.map_or_else(
                || (title.to_owned(), body.to_owned()),
                |values| rule.wording(title, body, values),
            );
            self.inner
                .database
                .queue_notification_delivery(rd_db::NewDelivery {
                    rule_id: rule.id,
                    target_id: rule.target_id,
                    // Derived from the rule and the event id, so a replay after a crash
                    // lands on the same key and the unique index drops it.
                    idempotency_key: rd_notify::idempotency_key(rule.id, event_id),
                    event: kind,
                    title,
                    body,
                    package_path: package_path.clone(),
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
    pub(super) async fn classify(
        &self,
        event: &rd_core::EventEnvelope,
    ) -> anyhow::Result<Option<Classified>> {
        match event.kind {
            rd_core::EventKind::PackageState => self.package_finished(&event.payload).await,
            rd_core::EventKind::StorageCapacity => {
                Ok(super::transitions::storage_notice(&event.payload)
                    .map(|(title, body)| (NotificationEvent::StorageBlocked, None, title, body)))
            }
            rd_core::EventKind::CaptchaChanged => {
                Ok(super::transitions::captcha_notice(&event.payload)
                    .map(|(title, body)| (NotificationEvent::CaptchaWaiting, None, title, body)))
            }
            rd_core::EventKind::PowerChanged => Ok(super::transitions::power_notice(
                &event.payload,
                event.occurred_at,
            )
            .map(|(title, body)| (NotificationEvent::PowerPending, None, title, body))),
            rd_core::EventKind::BandwidthChanged => Ok(budget_exhausted(&event.payload)),
            rd_core::EventKind::UsenetChanged => match usenet_quota_reached(&event.payload) {
                // One `usenet.changed` arm for both moments (RD-1100-05 quota, RD-1100-02 hopeless).
                Some(quota) => Ok(Some(quota)),
                None => self.usenet_job_hopeless(&event.payload).await,
            },
            rd_core::EventKind::QueueStopMark => self.stop_mark_reached(&event.payload).await,
            rd_core::EventKind::QueueState => Ok(super::transitions::queue_notice(&event.payload)
                .map(|(kind, title, body)| (kind, None, title, body))),
            rd_core::EventKind::DownloadState => self.stream_recorded(&event.payload).await,
            rd_core::EventKind::SubscriptionChanged => {
                self.subscription_matched(&event.payload).await
            }
            _ => Ok(None),
        }
    }

    /// A package that just finished or failed, decided on the event's payload (RD-1260-04): its
    /// row is read only for the name and the category, and a package removed in the meantime is
    /// still announced.
    async fn package_finished(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<Option<Classified>> {
        let Some(kind) = super::transitions::package_outcome(payload) else {
            return Ok(None);
        };
        // An id that does not parse names no row, exactly as it matched none before.
        let Some(package_id) = payload
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| id.parse::<rd_core::PackageId>().ok())
        else {
            return Ok(None);
        };
        let package = self.inner.database.get_package(package_id).await?;
        let category_id = package.as_ref().and_then(|package| package.category_id);
        let name = package.map_or_else(|| "A package".to_owned(), |package| package.name);
        if kind == NotificationEvent::PackageCompleted {
            return Ok(Some((
                kind,
                category_id,
                format!("Package finished: {name}"),
                format!("{name} ({})", PackageState::Completed),
            )));
        }
        // A package the malware scan stopped says so (RD-190-14): "failed" alone would read
        // like a broken archive, and this is the one failure somebody must not retry without
        // looking first.
        if let Some(finding) = self.malware_finding(&package_id.to_string()).await {
            return Ok(Some((
                kind,
                category_id,
                format!("Malware found: {name}"),
                format!("{name} ({finding})"),
            )));
        }
        Ok(Some((
            kind,
            category_id,
            format!("Package failed: {name}"),
            super::transitions::package_failed_body(payload, &name),
        )))
    }

    /// A livestream recording that finished (RD-1240-17), in its package's category. Every
    /// other `download.state` is a start (folded into a burst) or nobody's news.
    async fn stream_recorded(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<Option<Classified>> {
        if payload.get("state").and_then(|value| value.as_str()) != Some("completed") {
            return Ok(None);
        }
        let Some(id) = payload
            .get("download_id")
            .and_then(|value| value.as_str())
            .and_then(|id| id.parse::<rd_core::DownloadId>().ok())
        else {
            return Ok(None);
        };
        let database = &self.inner.database;
        let Some(file) = database.get_download(id).await? else {
            return Ok(None);
        };
        if file.kind != rd_core::DownloadKind::Record {
            return Ok(None);
        }
        let category_id = database
            .get_package(file.package_id)
            .await?
            .and_then(|package| package.category_id);
        Ok(Some((
            NotificationEvent::StreamRecorded,
            category_id,
            format!("Stream recorded: {}", file.file_name),
            format!("The recording {} is complete.", file.file_name),
        )))
    }

    /// A subscription poll that accepted new items (RD-1240-17), in the subscription's
    /// category. Every other `subscription.changed` - an edit, a poll that found nothing new -
    /// is nobody's news.
    async fn subscription_matched(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<Option<Classified>> {
        if payload.get("poll").and_then(|value| value.as_str()) != Some("finished") {
            return Ok(None);
        }
        let accepted = payload
            .get("accepted")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if accepted == 0 {
            return Ok(None);
        }
        let subscription = match payload
            .get("subscription_id")
            .and_then(|value| value.as_str())
            .and_then(|id| id.parse::<rd_core::SubscriptionId>().ok())
        {
            Some(id) => self.inner.database.subscription(id).await?,
            None => None,
        };
        let (name, category_id) = subscription.map_or_else(
            || ("A subscription".to_owned(), None),
            |subscription| (subscription.name, subscription.category_id),
        );
        let items = if accepted == 1 {
            "1 new item".to_owned()
        } else {
            format!("{accepted} new items")
        };
        Ok(Some((
            NotificationEvent::SubscriptionMatched,
            category_id,
            format!("New in {name}: {items}"),
            format!("The check of {name} accepted {items}."),
        )))
    }

    /// The queue paused at its stop mark (RD-1210-02), in the marked package's category. A mark
    /// set or cleared by hand is nobody's news.
    async fn stop_mark_reached(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<Option<Classified>> {
        if payload.get("action").and_then(|value| value.as_str()) != Some("reached") {
            return Ok(None);
        }
        let id = |key: &str| payload.get(key).and_then(|value| value.as_str());
        let database = &self.inner.database;
        let (name, package) = if let Some(id) =
            id("download_id").and_then(|id| id.parse::<rd_core::DownloadId>().ok())
        {
            match database.get_download(id).await? {
                Some(file) => (
                    Some(file.file_name),
                    database.get_package(file.package_id).await?,
                ),
                None => (None, None),
            }
        } else if let Some(id) =
            id("package_id").and_then(|id| id.parse::<rd_core::PackageId>().ok())
        {
            let package = database.get_package(id).await?;
            (
                package.as_ref().map(|package| package.name.clone()),
                package,
            )
        } else {
            (None, None)
        };
        let name = name.unwrap_or_else(|| "the marked download".to_owned());
        Ok(Some((
            NotificationEvent::StopMarkReached,
            package.and_then(|package| package.category_id),
            format!("Stop mark reached: {name}"),
            format!("The queue paused after {name}. Resume it to go on."),
        )))
    }

    /// A Usenet set given up as beyond repair (RD-1100-02), in its package's category.
    ///
    /// Every other `usenet.changed` - an import arriving, a server edited - is not a moment
    /// anybody is told about.
    async fn usenet_job_hopeless(
        &self,
        payload: &serde_json::Value,
    ) -> anyhow::Result<Option<Classified>> {
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
