//! The event side: which events become notifications, and for whom.

use super::*;

/// What a delivery keeps besides its words: the finished package's folder (RD-1250-01) and what
/// a webhook is told it is about (RD-1480-11).
#[derive(Default)]
pub(super) struct About {
    pub(super) package_path: Option<String>,
    pub(super) subject: Subject,
}

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
            About {
                package_path,
                subject: self.subject(event).await?,
            },
            values.as_ref(),
        )
        .await
    }

    /// What a bus event is about (RD-1480-11): its moment, and the package and the file its
    /// payload names. A file's package is read when the payload names the file alone.
    async fn subject(&self, event: &rd_core::EventEnvelope) -> anyhow::Result<Subject> {
        let text = |key: &str| event.payload.get(key).and_then(serde_json::Value::as_str);
        let download_id = text("download_id").and_then(|id| id.parse::<rd_core::DownloadId>().ok());
        let mut package_id =
            text("package_id").and_then(|id| id.parse::<rd_core::PackageId>().ok());
        if let (None, Some(id)) = (package_id, download_id) {
            package_id = self
                .inner
                .database
                .get_download(id)
                .await?
                .map(|file| file.package_id);
        }
        Ok(Subject {
            occurred_at: Some(event.occurred_at),
            package_id,
            download_id,
        })
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

    /// Queues one delivery of an event for every rule that wants it, each rule with a template
    /// sending its own wording filled with `values` (RD-1280-02); without values, every rule
    /// sends the built-in one, in the language of the settings (RD-1480-11). `about` is kept with
    /// each delivery so a retry says the same.
    pub(super) async fn queue_worded(
        &self,
        kind: NotificationEvent,
        category_id: Option<rd_core::CategoryId>,
        event_id: &str,
        (title, body): (&Text, &Text),
        about: About,
        values: Option<&rd_notify::Values>,
    ) -> anyhow::Result<()> {
        let rules = self.inner.database.list_notification_rules().await?;
        if !rules.iter().any(|rule| rule.matches(kind, category_id)) {
            return Ok(());
        }
        let language = notification_language(&self.inner.database).await;
        let (title, body) = (title.say(language), body.say(language));
        for rule in rules {
            if !rule.matches(kind, category_id) {
                continue;
            }
            let (title, body) = values.map_or_else(
                || (title.clone(), body.clone()),
                |values| rule.wording(&title, &body, values),
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
                    package_path: about.package_path.clone(),
                    subject: about.subject,
                })
                .await?;
        }
        Ok(())
    }

    /// What the malware scan found in a package, as one line, when a finding is what stopped it.
    pub(super) async fn malware_finding(&self, package_id: &str) -> Option<Words> {
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
                .map_or_else(|| Text::new("package_malware.finding").into(), Words::from),
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
        let name: Words = package.map_or_else(
            || Text::new("package.unnamed").into(),
            |package| package.name.into(),
        );
        if kind == NotificationEvent::PackageCompleted {
            return Ok(Some((
                kind,
                category_id,
                Text::new("package_completed.title").with("name", name.clone()),
                Text::new("package_completed.body").with("name", name),
            )));
        }
        // A package the malware scan stopped says so (RD-190-14): "failed" alone would read
        // like a broken archive, and this is the one failure somebody must not retry without
        // looking first.
        if let Some(finding) = self.malware_finding(&package_id.to_string()).await {
            return Ok(Some((
                kind,
                category_id,
                Text::new("package_malware.title").with("name", name.clone()),
                Text::new("package_malware.body")
                    .with("name", name)
                    .with("finding", finding),
            )));
        }
        Ok(Some((
            kind,
            category_id,
            Text::new("package_failed.title").with("name", name.clone()),
            super::transitions::package_failed_body(payload, name),
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
            Text::new("stream_recorded.title").with("name", file.file_name.clone()),
            Text::new("stream_recorded.body").with("name", file.file_name),
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
        let (name, category_id): (Words, _) = subscription.map_or_else(
            || (Text::new("subscription_matched.unnamed").into(), None),
            |subscription| (subscription.name.into(), subscription.category_id),
        );
        Ok(Some((
            NotificationEvent::SubscriptionMatched,
            category_id,
            Text::counted("subscription_matched.title", accepted).with("name", name.clone()),
            Text::counted("subscription_matched.body", accepted).with("name", name),
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
        let name: Words = name.map_or_else(|| Text::new("stop_mark.unnamed").into(), Words::from);
        Ok(Some((
            NotificationEvent::StopMarkReached,
            package.and_then(|package| package.category_id),
            Text::new("stop_mark.title").with("name", name.clone()),
            Text::new("stop_mark.body").with("name", name),
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
        let name: Words = package.as_ref().map_or_else(
            || Text::new("usenet_hopeless.unnamed").into(),
            |package| package.name.clone().into(),
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
            Text::new("usenet_hopeless.title").with("name", name.clone()),
            Text::new("usenet_hopeless.body")
                .with("name", name)
                .with("missing", count("missing_blocks"))
                .with("available", count("available_blocks")),
        )))
    }
}
