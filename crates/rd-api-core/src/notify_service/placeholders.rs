//! What a rule's own wording is filled with (RD-1280-02).
//!
//! Read only when a rule that wants the event words its notifications itself, so an
//! installation without templates pays nothing. The values follow the table in `rd_notify`
//! (`NotificationEvent::placeholders`): what an event's payload names, and the rows it points
//! at. A row gone in the meantime leaves its placeholders empty, never the delivery unqueued. An
//! error is redacted like every failure the service shows, so no address with a key in it
//! reaches a chat. The traffic budget is the one of the moment the delivery is queued, read from
//! the bandwidth state (RD-1290-02), and a retry sends what the first attempt did. A burst of
//! starts or of added links is filled when it closes, from the burst itself (RD-1291-01).

use rd_notify::{Placeholder as P, Values};

use super::*;

impl NotificationService {
    /// The placeholder values of a bus event, `None` when no matching rule has a template.
    pub(super) async fn wanted_values(
        &self,
        kind: NotificationEvent,
        category_id: Option<rd_core::CategoryId>,
        event: &rd_core::EventEnvelope,
    ) -> anyhow::Result<Option<Values>> {
        if !self.worded(kind, category_id).await? {
            return Ok(None);
        }
        // Time and date in the service's own zone, the one its schedules are read in.
        let at = event.occurred_at.with_timezone(&chrono::Local);
        let mut values = Values::stamped(kind, &at);
        set_budget(&mut values, self.inner.bandwidth.budget_reading().await);
        self.fill_values(kind, &event.payload, &mut values).await?;
        Ok(Some(values))
    }

    /// The placeholder values of a closed burst, `None` when no matching rule has a template:
    /// what the burst counted and named, and for a burst of one start its package and category,
    /// as a single event fills them. Several starts leave those two empty.
    pub(super) async fn burst_values(
        &self,
        burst: &rd_notify::Burst,
    ) -> anyhow::Result<Option<Values>> {
        if !self.worded(burst.event, burst.category_id).await? {
            return Ok(None);
        }
        let mut values = burst.values(&chrono::Local);
        set_budget(&mut values, self.inner.bandwidth.budget_reading().await);
        if burst.single()
            && let Some(id) = burst.package_id
            && let Some(package) = self.inner.database.get_package(id).await?
        {
            values.set(P::Package, package.name);
            if let Some(category) = package.category_id {
                values.set(P::Category, self.category_name(category).await?);
            }
        }
        Ok(Some(values))
    }

    /// Whether a rule that wants the event words its notifications itself.
    async fn worded(
        &self,
        kind: NotificationEvent,
        category_id: Option<rd_core::CategoryId>,
    ) -> anyhow::Result<bool> {
        if !kind.takes_template() {
            return Ok(false);
        }
        Ok(self
            .inner
            .database
            .list_notification_rules()
            .await?
            .iter()
            .any(|rule| rule.has_template() && rule.matches(kind, category_id)))
    }

    async fn fill_values(
        &self,
        kind: NotificationEvent,
        payload: &serde_json::Value,
        values: &mut Values,
    ) -> anyhow::Result<()> {
        let text = |key: &str| payload.get(key).and_then(serde_json::Value::as_str);
        let package_id = text("package_id").and_then(|id| id.parse::<rd_core::PackageId>().ok());
        let download_id = text("download_id").and_then(|id| id.parse::<rd_core::DownloadId>().ok());
        match kind {
            NotificationEvent::PackageCompleted
            | NotificationEvent::PackageFailed
            | NotificationEvent::UsenetJobHopeless => {
                let Some(id) = package_id else {
                    return Ok(());
                };
                let folder = self.package_values(id, values).await?;
                values.set(P::State, text("state").unwrap_or_default());
                if kind == NotificationEvent::PackageCompleted {
                    values.set(P::Path, folder.unwrap_or_default());
                }
                if kind == NotificationEvent::PackageFailed {
                    values.set(P::Error, self.package_error(id).await?);
                }
            }
            NotificationEvent::StreamRecorded | NotificationEvent::StopMarkReached => {
                if let Some(id) = download_id {
                    if let Some(file) = self.inner.database.get_download(id).await? {
                        self.package_values(file.package_id, values).await?;
                        values.set(P::File, file.file_name.clone());
                        let bytes = file
                            .total_bytes
                            .map_or(file.committed_bytes.get(), |total| total.get());
                        if bytes > 0 {
                            values.set(P::Size, rd_notify::size_text(bytes));
                        }
                    }
                } else if let Some(id) = package_id {
                    self.package_values(id, values).await?;
                }
            }
            NotificationEvent::StorageBlocked => {
                values.set(P::StorageTarget, text("name").unwrap_or_default());
                values.set(P::Path, text("path").unwrap_or_default());
            }
            NotificationEvent::QueuePaused => {
                values.set(P::Reason, text("reason").unwrap_or_default());
            }
            NotificationEvent::SubscriptionMatched => {
                let subscription = match text("subscription_id")
                    .and_then(|id| id.parse::<rd_core::SubscriptionId>().ok())
                {
                    Some(id) => self.inner.database.subscription(id).await?,
                    None => None,
                };
                if let Some(category) = subscription.and_then(|found| found.category_id) {
                    values.set(P::Category, self.category_name(category).await?);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Name, category and size of a package; answers its folder.
    async fn package_values(
        &self,
        id: rd_core::PackageId,
        values: &mut Values,
    ) -> anyhow::Result<Option<String>> {
        let database = &self.inner.database;
        let Some(package) = database.get_package(id).await? else {
            return Ok(None);
        };
        values.set(P::Package, package.name.clone());
        if let Some(category) = package.category_id {
            values.set(P::Category, self.category_name(category).await?);
        }
        let bytes: u64 = database
            .downloads_for_package(id)
            .await?
            .iter()
            .map(|file| {
                file.total_bytes
                    .map_or(file.committed_bytes.get(), |total| total.get())
            })
            .sum();
        // Nothing known is no size, rather than a "0 B" that reads like an empty download.
        if bytes > 0 {
            values.set(P::Size, rd_notify::size_text(bytes));
        }
        Ok(Some(package.destination).filter(|folder| !folder.trim().is_empty()))
    }

    /// What stopped a failed package: the malware scan's finding, else its first failed file's.
    async fn package_error(&self, id: rd_core::PackageId) -> anyhow::Result<String> {
        if let Some(finding) = self.malware_finding(&id.to_string()).await {
            return Ok(rd_core::redact_text(&finding));
        }
        Ok(self
            .inner
            .database
            .downloads_for_package(id)
            .await?
            .into_iter()
            .filter(|file| file.state == rd_core::DownloadState::Failed)
            .find_map(|file| file.last_error)
            .map(|failure| rd_core::redact_text(&failure.message))
            .unwrap_or_default())
    }

    async fn category_name(&self, id: rd_core::CategoryId) -> anyhow::Result<String> {
        Ok(self
            .inner
            .database
            .list_categories()
            .await?
            .into_iter()
            .find(|category| category.id == id)
            .map(|category| category.name)
            .unwrap_or_default())
    }
}

/// The budget's four placeholders (RD-1290-02); without an active budget they stay empty.
fn set_budget(values: &mut Values, reading: Option<rd_limits::BudgetReading>) {
    let Some(reading) = reading else {
        return;
    };
    values.set(P::BudgetUsed, rd_notify::size_text(reading.used_bytes));
    values.set(P::BudgetLeft, rd_notify::size_text(reading.left_bytes()));
    values.set(P::BudgetLimit, rd_notify::size_text(reading.limit_bytes));
    values.set(
        P::BudgetPeriod,
        match reading.period {
            rd_limits::BudgetKind::Daily => "daily",
            rd_limits::BudgetKind::Monthly => "monthly",
        },
    );
}

#[cfg(test)]
#[path = "placeholders_tests.rs"]
mod tests;
