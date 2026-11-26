//! The events whose bursts are one notification each (RD-1240-17): links arriving in the
//! LinkGrabber and downloads starting. Read off the bus here, folded by
//! `rd_notify::Coalescer`, and queued when a burst closes - a hundred links pasted one after
//! another reach a target as one message, not a hundred. A rule's own wording is filled then,
//! once per burst and rule (RD-1291-01).

use super::*;

/// How often the open bursts are looked at.
pub(super) const BURST_TICK: Duration = Duration::from_secs(1);

/// Which coalescing event a bus event is, read off its payload alone.
///
/// A download counts as started when it leaves the queue or its resolver for the transfer; a
/// resume after a pause or a retry wait is the same download going on, not news.
pub(super) fn coalescing_kind(event: &rd_core::EventEnvelope) -> Option<NotificationEvent> {
    let text = |key: &str| event.payload.get(key).and_then(serde_json::Value::as_str);
    match event.kind {
        rd_core::EventKind::CollectorIntake => Some(NotificationEvent::LinksAdded),
        rd_core::EventKind::DownloadState
            if text("state") == Some("downloading")
                && matches!(text("previous"), Some("queued" | "resolving")) =>
        {
            Some(NotificationEvent::DownloadStarted)
        }
        _ => None,
    }
}

impl NotificationService {
    /// Folds one occurrence into its burst - read further only when a rule asks for it, since
    /// these events come with every download and every intake.
    pub(super) async fn fold_into_burst(
        &self,
        kind: NotificationEvent,
        event: &rd_core::EventEnvelope,
    ) -> anyhow::Result<()> {
        let rules = self.inner.database.list_notification_rules().await?;
        // The rule's own category passes the category check: whether a download's category
        // fits is decided when the burst is queued.
        if !rules
            .iter()
            .any(|rule| rule.matches(kind, rule.category_id))
        {
            return Ok(());
        }
        let payload = &event.payload;
        let occurrence = match kind {
            NotificationEvent::LinksAdded => {
                let items = payload
                    .get("candidate_count")
                    .or_else(|| payload.get("file_count"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                rd_notify::Occurrence {
                    event: kind,
                    category_id: None,
                    event_id: event.id.to_string(),
                    items,
                    name: payload
                        .get("source")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    occurred_at: event.occurred_at,
                    package_id: None,
                    download_id: None,
                }
            }
            _ => {
                let Some(id) = payload
                    .get("download_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| id.parse::<rd_core::DownloadId>().ok())
                else {
                    return Ok(());
                };
                let database = &self.inner.database;
                let Some(file) = database.get_download(id).await? else {
                    return Ok(());
                };
                let category_id = database
                    .get_package(file.package_id)
                    .await?
                    .and_then(|package| package.category_id);
                rd_notify::Occurrence {
                    event: kind,
                    category_id,
                    event_id: event.id.to_string(),
                    items: 1,
                    name: Some(file.file_name),
                    occurred_at: event.occurred_at,
                    package_id: Some(file.package_id),
                    download_id: Some(id),
                }
            }
        };
        self.inner
            .bursts
            .lock()
            .await
            .push(occurrence, rd_core::Timestamp::now());
        Ok(())
    }

    /// Queues the bursts that closed - all of them when the service stops. The wording comes
    /// from the burst alone and its first bus event's time, so a burst queued again after a
    /// restart says what it said before, and the idempotency key keeps it to one delivery.
    pub(super) async fn flush_bursts(&self, all: bool) {
        let closed = {
            let mut bursts = self.inner.bursts.lock().await;
            if bursts.is_empty() {
                return;
            }
            if all {
                bursts.drain()
            } else {
                bursts.due(rd_core::Timestamp::now())
            }
        };
        for burst in closed {
            let (title, body) = burst_text(&burst);
            let values = match self.burst_values(&burst).await {
                Ok(values) => values,
                Err(error) => {
                    // The built-in wording still goes out rather than nothing.
                    tracing::warn!(%error, "notification burst placeholders could not be read");
                    None
                }
            };
            if let Err(error) = self
                .queue_worded(
                    burst.event,
                    burst.category_id,
                    &burst.first_event_id,
                    (&title, &body),
                    super::events::About {
                        package_path: None,
                        subject: burst.subject(),
                    },
                    values.as_ref(),
                )
                .await
            {
                tracing::warn!(%error, "notification burst could not be queued");
            }
        }
    }
}

/// The title and the body of a closed burst.
pub(super) fn burst_text(burst: &rd_notify::Burst) -> (Text, Text) {
    let names = burst.names();
    match burst.event {
        NotificationEvent::LinksAdded => {
            let title = if burst.occurrences == 1 {
                Text::counted("links_added.title", burst.items)
            } else {
                Text::counted("links_added.title_imports", burst.items)
                    .with("imports", burst.occurrences)
            };
            let body = if burst.names.is_empty() {
                Text::counted("links_added.body", burst.items)
            } else {
                Text::counted("links_added.body_from", burst.items).with("names", names)
            };
            (title, body)
        }
        _ => {
            let title = match burst.names.as_slice() {
                [only] if burst.items == 1 => {
                    Text::new("download_started.title").with("name", only.as_str())
                }
                _ => Text::counted("download_started.title_count", burst.items),
            };
            (
                title,
                Text::new("download_started.body").with("names", names),
            )
        }
    }
}

#[cfg(test)]
#[path = "bursts_tests.rs"]
mod tests;
