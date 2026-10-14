//! Database facade for what the diagnostic bundle reads about notifications and the queue
//! (RD-1291-04). Reads only; see `diagnostics_store` for what they leave out.

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::{
    Database, diagnostics_store,
    diagnostics_store::{NotificationDiagnostics, QueueDiagnostics},
};

impl Database {
    /// Notification targets and rules without endpoints or wording, the deliveries queued
    /// since `since` per event and state, and the newest `recent` deliveries.
    pub async fn notification_diagnostics(
        &self,
        since: DateTime<Utc>,
        recent: u32,
    ) -> Result<NotificationDiagnostics> {
        diagnostics_store::notification_diagnostics(&self.readers, since, recent).await
    }

    /// Packages per state and stage, the newest `recent` package transitions and the downloads
    /// per state and failure, without a name, a folder or an address.
    pub async fn queue_diagnostics(&self, recent: u32) -> Result<QueueDiagnostics> {
        diagnostics_store::queue_diagnostics(&self.readers, recent).await
    }
}
