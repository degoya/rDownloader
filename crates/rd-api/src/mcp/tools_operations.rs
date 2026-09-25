//! MCP tools for how the installation is doing: power, reconnect, metrics, the diagnostic
//! bundle's preview, plugin runs and catalogues, remote-job providers and an account's hosters
//! (RD-120-55).
//!
//! Each was out under RD-120-29 as "a panel", "the form drawing itself" or "the person at the
//! machine's business". Checked against the owner's line instead, none of these hands out a
//! secret, takes one in, gives a consent or changes something outside this machine
//! irreversibly. What does is not here, and `super::coverage` says so row by row: approving
//! and fetching a diagnostic bundle (the approval is the person's), and reconnecting (a new
//! public address, and the old one is gone). The power tools can read a countdown and stop
//! one; nothing here starts a standby or a shutdown.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    tool, tool_router,
};

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, json_result, parse_id, respond},
    params_config::IdParams,
    params_remaining::PluginMessagesParams,
};
use crate::ApiError;

#[tool_router(router = operations_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read the power state: which completion action (standby, shutdown, ...) is set and whether it lacks the local approval it needs, whether one is counting down right now and when it fires, whether quiet hours are in force and until when, and whether the machine is being kept awake."
    )]
    pub async fn get_power_status(&self) -> McpToolResult {
        let Json(status) = crate::power_handlers::power_status(State(self.state.clone())).await;
        json_result(&status)
    }

    #[tool(
        description = "Stop a standby or shutdown that is counting down, so the machine stays on. The cycle counts as handled and does not start again by itself. Answers power.nothing_pending when nothing was counting down. Whether a finished queue may power the machine down at all is a setting (update_settings)."
    )]
    pub async fn cancel_power_action(&self) -> McpToolResult {
        respond(
            crate::power_handlers::cancel_power_action(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read the reconnect state: whether reconnecting is switched on, whether one is running, how the last attempt went, when the next may run, and which hosters are holding back for an address limit. Reconnecting itself is not available here; it changes the public address, which cannot be undone."
    )]
    pub async fn get_reconnect_status(&self) -> McpToolResult {
        respond(
            crate::reconnect_handlers::reconnect_status(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read the Prometheus metrics exposition as text: queue gauges, transfer counters, uptime. Costs the api:metrics permission, like the scrape route."
    )]
    pub async fn get_metrics(&self) -> McpToolResult {
        let result = async {
            let response =
                crate::metrics::scrape_metrics(State(self.state.clone()), HeaderMap::new()).await?;
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .map_err(anyhow::Error::new)?;
            Ok::<_, ApiError>(String::from_utf8_lossy(&bytes).into_owned())
        }
        .await;
        // Text rather than a JSON string: the exposition is line-oriented, and escaping every
        // newline would make it unreadable to the caller it is for.
        match result {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            Err(error) => Ok(api_error(error)),
        }
    }

    #[tool(
        description = "Preview what a diagnostic bundle would contain: each entry, how many items it holds, what was redacted from it, and what no bundle ever contains. Only a preview: approving and writing the bundle is the person's, in the web UI."
    )]
    pub async fn preview_diagnostic_bundle(&self) -> McpToolResult {
        respond(
            crate::diagnostics_handlers::preview_diagnostic_bundle(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List one plugin's newest recorded runs (id from list_configuration section plugins): operation, outcome (ok, failed, crash, timeout, ...), stable error class, a redacted message, when and how long. The correlation_id matches list_log_records."
    )]
    pub async fn list_plugin_executions(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_handlers::list_plugin_executions(
                State(self.state.clone()),
                Path(params.id),
            )
            .await
            .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read the installed plugins' own message catalogue for one language: the text behind the stable codes a plugin answers with (its settings labels, its error codes)."
    )]
    pub async fn get_plugin_messages(
        &self,
        Parameters(params): Parameters<PluginMessagesParams>,
    ) -> McpToolResult {
        respond(
            crate::plugin_handlers::plugin_messages(State(self.state.clone()), Path(params.locale))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List the provider services an installed plugin can run a remote job for. An account whose provider is not listed is refused by submit_remote_job with remote_job.no_plugin."
    )]
    pub async fn list_remote_job_providers(&self) -> McpToolResult {
        respond(
            crate::remote_job_handlers::list_remote_job_providers(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List the hoster domains one premium account can download from (id from list_configuration section accounts), as its provider reports them; cached for an hour. No credential is included."
    )]
    pub async fn list_account_hosters(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(answer) =
                crate::hosters::list_account_hosters(State(self.state.clone()), Path(id)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }
}
