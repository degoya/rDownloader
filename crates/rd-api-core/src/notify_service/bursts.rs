//! The events whose bursts are one notification each (RD-1240-17): links arriving in the
//! LinkGrabber and downloads starting. Read off the bus here, folded by
//! `rd_notify::Coalescer`, and queued when a burst closes - a hundred links pasted one after
//! another reach a target as one message, not a hundred.

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
                        .map(|source| source_label(source).to_owned()),
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
                }
            }
        };
        self.inner
            .bursts
            .lock()
            .await
            .push(occurrence, chrono::Utc::now());
        Ok(())
    }

    /// Queues the bursts that closed - all of them when the service stops.
    pub(super) async fn flush_bursts(&self, all: bool) {
        let closed = {
            let mut bursts = self.inner.bursts.lock().await;
            if bursts.is_empty() {
                return;
            }
            if all {
                bursts.drain()
            } else {
                bursts.due(chrono::Utc::now())
            }
        };
        for burst in closed {
            let (title, body) = burst_text(&burst);
            if let Err(error) = self
                .queue_event(
                    burst.event,
                    burst.category_id,
                    &burst.first_event_id,
                    &title,
                    &body,
                )
                .await
            {
                tracing::warn!(%error, "notification burst could not be queued");
            }
        }
    }
}

/// The title and the body of a closed burst.
pub(super) fn burst_text(burst: &rd_notify::Burst) -> (String, String) {
    let named = burst.names.join(", ");
    match burst.event {
        NotificationEvent::LinksAdded => {
            let links = counted(burst.items, "link", "links");
            let title = if burst.occurrences == 1 {
                format!("{links} added")
            } else {
                format!(
                    "{links} added in {}",
                    counted(burst.occurrences, "import", "imports")
                )
            };
            let body = if named.is_empty() {
                format!("{links} arrived in the LinkGrabber.")
            } else {
                format!("{links} arrived in the LinkGrabber from: {named}.")
            };
            (title, body)
        }
        _ => {
            let title = match burst.names.as_slice() {
                [only] if burst.items == 1 => format!("Download started: {only}"),
                _ => format!("{} started", counted(burst.items, "download", "downloads")),
            };
            let rest = burst
                .items
                .saturating_sub(u64::try_from(burst.names.len()).unwrap_or(u64::MAX));
            let body = if rest == 0 {
                format!("Started: {named}.")
            } else {
                format!("Started: {named} and {rest} more.")
            };
            (title, body)
        }
    }
}

/// `1 link`, `5 links`.
fn counted(count: u64, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// Where an intake came from, as the text says it (`rd_core::IngressSource`'s wire names).
pub(super) fn source_label(source: &str) -> &str {
    match source {
        "manual" => "added by hand",
        "clipboard" => "clipboard",
        "click_and_load" => "Click'n'Load",
        "api" => "API",
        "nzb" => "NZB file",
        "hot_folder" => "hot folder",
        "browser_extension" => "browser extension",
        "browser_download" => "browser download",
        "subscription" => "subscription",
        other => other,
    }
}

#[cfg(test)]
#[path = "bursts_tests.rs"]
mod tests;
