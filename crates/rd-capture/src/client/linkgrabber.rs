//! The tray's "Add all from LinkGrabber" (RD-1240-07, `crate::linkgrabber`).

use anyhow::Result;

use super::{CaptureClient, ensure_success};
use crate::linkgrabber::Enqueued;

impl CaptureClient {
    /// Moves everything the LinkGrabber holds into the queue, started or `paused`, as the web
    /// interface's `E` and `W` do.
    ///
    /// Refused with `auth.scope_insufficient` unless the agent was paired with queue control,
    /// like the queue entries beside it.
    pub(crate) async fn enqueue_linkgrabber(&self, paused: bool) -> Result<Enqueued> {
        let endpoint = self.service.join("api/v1/capture/linkgrabber/enqueue")?;
        let body = serde_json::json!({ "paused": paused });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        let response = ensure_success(response, "LinkGrabber enqueue").await?;
        Ok(response.json().await?)
    }
}
