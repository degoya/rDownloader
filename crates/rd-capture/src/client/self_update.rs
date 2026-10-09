//! The agent's own update on its settings poll (RD-1210-03, `rd_update::agent::report`): the
//! report goes out as a header, the service's channel and a request to install come back as
//! headers.

use super::CaptureClient;

impl CaptureClient {
    /// What the agent's own update shares with this client.
    pub(crate) fn self_update(&self) -> &crate::self_update::Shared {
        &self.self_update
    }

    /// `request` with the agent's update report, once there is one.
    pub(super) fn with_report(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.self_update.report() {
            Some(report) => request.header(rd_update::agent::report::REPORT_HEADER, report),
            None => request,
        }
    }
}
