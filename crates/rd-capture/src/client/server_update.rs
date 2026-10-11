//! The service's own update in the tray (RD-1240-25, `crate::server_update`), and its restart
//! (RD-1240-32).

use anyhow::Result;

use super::{CaptureClient, ensure_success};
use crate::server_update::{Installing, Reading};

impl CaptureClient {
    /// Whether the service has an update, how it is installed, whether this agent may install it
    /// and where an install stands. Every capture token may read it.
    pub(crate) async fn server_update(&self) -> Result<Reading> {
        let url = self.service.join("api/v1/capture/server-update")?;
        let response = self.send(self.http.get(url)).await?;
        let response = ensure_success(response, "server update").await?;
        Ok(response.json().await?)
    }

    /// Starts the install the web interface starts; `allow_active` installs while downloads
    /// run, which the stop saves and the restart continues.
    ///
    /// Refused with `auth.scope_insufficient` unless the agent was paired with
    /// `capture:server_update`, and with the install's own codes (`update.transfers_active`,
    /// `update.install_unsupported`, ...) like the web interface's request.
    pub(crate) async fn install_server_update(&self, allow_active: bool) -> Result<Installing> {
        let endpoint = self.service.join("api/v1/capture/server-update/install")?;
        let body = serde_json::json!({ "allow_active": allow_active });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        let response = ensure_success(response, "server update install").await?;
        Ok(response.json().await?)
    }

    /// Restarts the service for what waits for the next start (RD-1240-32), as the web
    /// interface's button does; `allow_active` restarts while downloads run. Answers how the
    /// service comes back: `self`, `supervisor` or `manual`.
    ///
    /// Refused with `auth.scope_insufficient` unless the agent was paired with
    /// `capture:server_update`, and with the restart's own codes (`restart.transfers_active`,
    /// `restart.update_running`, `restart.already_restarting`).
    pub(crate) async fn restart_server(&self, allow_active: bool) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct Started {
            how: String,
        }
        let endpoint = self.service.join("api/v1/capture/server-update/restart")?;
        let body = serde_json::json!({ "allow_active": allow_active });
        let response = self.send(self.http.post(endpoint).json(&body)).await?;
        let response = ensure_success(response, "server restart").await?;
        Ok(response.json::<Started>().await?.how)
    }
}
