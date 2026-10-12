//! MCP tools for the level the service logs at and a debug window (RD-1260-12).
//!
//! The same four operations as the Log view: read where things stand, set the level, put one
//! area on debug for a while, end that early. A window ends by itself, so an assistant that
//! opens one and forgets about it leaves nothing behind for good.

use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_handling::body,
};
use crate::log_level_handlers;

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SetLogLevelParams {
    /// `error`, `warn`, `info` (the default) or `debug`.
    pub level: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct StartLogDebugParams {
    /// plugin, provider, usenet, torrent, media, linkgrabber, subscriptions, http or scheduler.
    pub area: String,
    /// Only with `area: plugin`: an installed plugin's id (list_configuration, section plugins), to put only that
    /// plugin's own lines on debug.
    #[serde(default)]
    pub plugin_id: Option<String>,
    /// 15, 30 or 60.
    pub minutes: u32,
}

#[tool_router(router = log_level_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read the level the service logs at (`level`: error, warn, info - the default - or debug; console, stored log and trace export alike), the debug window that is open (`debug`: area, plugin_id, until; null when none) and `rust_log`: the RUST_LOG the service was started with, which decides the filter instead while set (the level and the window cannot be changed then). `areas` and `minutes` list what start_log_debug takes."
    )]
    pub async fn get_log_level(&self) -> McpToolResult {
        respond(log_level_handlers::log_level_state(&self.state).await)
    }

    #[tool(
        description = "Set the level the service logs at: error, warn, info (the default) or debug. Saved in the settings (the log_level key of update_settings) and in force from the next line, without a restart. Answers diagnostics.log_level_locked naming rust_log while the service runs with RUST_LOG."
    )]
    pub async fn set_log_level(
        &self,
        Parameters(params): Parameters<SetLogLevelParams>,
    ) -> McpToolResult {
        let result = async {
            let request: log_level_handlers::LogLevelRequest =
                body(serde_json::json!({ "level": params.level }))?;
            let holds_admin = super::granted_now().contains(&rd_core::Scope::Admin);
            log_level_handlers::change_log_level(
                &self.state,
                &crate::audit::AuditContext::current(),
                holds_admin,
                request.level,
            )
            .await
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Put one area of the service on debug for 15, 30 or 60 minutes while the rest keeps its level, to see in list_log_records what it does: plugin (the plugin host and every plugin's own lines, or with plugin_id only that plugin's own lines), provider (hosters and the resolver plugins), usenet, torrent, media, linkgrabber, subscriptions, http or scheduler. It ends by itself, also across a restart; opening another replaces it. Answers diagnostics.debug_minutes_invalid, diagnostics.debug_plugin_invalid (a plugin_id with another area or not installed) or diagnostics.log_level_locked (RUST_LOG set). Secrets stay redacted on debug."
    )]
    pub async fn start_log_debug(
        &self,
        Parameters(params): Parameters<StartLogDebugParams>,
    ) -> McpToolResult {
        let result = async {
            let request: log_level_handlers::LogDebugRequest = body(serde_json::json!({
                "area": params.area,
                "plugin_id": params.plugin_id,
                "minutes": params.minutes,
            }))?;
            log_level_handlers::open_debug_window(&self.state, request).await
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "End the open debug window now (start_log_debug); the level holds again from the next line. Nothing open is no error."
    )]
    pub async fn stop_log_debug(&self) -> McpToolResult {
        respond(log_level_handlers::close_debug_window(&self.state).await)
    }
}
