//! MCP tools for the download window of a package and of a category (RD-1240-30).
//!
//! Each calls its route's handler, so the checks and the stable codes are the route's. Whether
//! the bandwidth schedule pauses downloads is a profile's `pause_downloads`, read with
//! list_bandwidth_profiles and get_bandwidth_status; profiles are edited in the interface.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_config::IdParams,
    params_handling::body,
};
use crate::{config_handlers, error_codes::parse_id, package_handlers as packages};

/// One weekly span of a download window.
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
pub(crate) struct WindowSpanParam {
    /// Weekdays as a Monday-first bitmask: 1 = Monday, 2 = Tuesday, 4 = Wednesday, … 64 =
    /// Sunday; 127 = every day.
    pub days: u8,
    /// Start, in minutes after local midnight in the bandwidth schedule's timezone (0–1439).
    pub start_minute: u16,
    /// Exclusive end in minutes (1–1440); below start_minute the span wraps past midnight.
    pub end_minute: u16,
}

/// Sets or removes a download window.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DownloadWindowParams {
    /// Package id (from list_packages) or category id (from list_configuration, section
    /// categories), as the tool says.
    pub id: String,
    /// The times the files may download, at most 28 spans; empty or absent means any time, so
    /// only ignore_schedule_pause counts.
    #[serde(default)]
    pub windows: Vec<WindowSpanParam>,
    /// Download even while the bandwidth profile in force pauses downloads.
    #[serde(default)]
    pub ignore_schedule_pause: bool,
    /// true removes the window instead (the other fields are then ignored).
    #[serde(default)]
    pub remove: bool,
}

impl DownloadWindowParams {
    /// The REST body: the window, or `null` to remove it.
    fn request(&self) -> serde_json::Value {
        if self.remove {
            return serde_json::json!({ "download_window": null });
        }
        serde_json::json!({
            "download_window": {
                "windows": self.windows,
                "ignore_schedule_pause": self.ignore_schedule_pause,
            }
        })
    }
}

#[tool_router(router = download_window_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read one download package's download window (id from list_packages): download_window, its own (null = it follows its category's), category_window, and held: why its files wait right now — window (outside the window that applies), schedule (the bandwidth profile in force has pause_downloads and the package does not ignore it) — or null while they may download."
    )]
    pub async fn get_package_download_window(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(status) =
                packages::get_package_download_window(State(self.state.clone()), Path(id)).await?;
            Ok(status)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Set when one download package may download (id from list_packages): weekly windows in the bandwidth schedule's timezone — outside them its waiting files wait and its running transfers that can resume pause, and continue once a window opens — and ignore_schedule_pause, so it downloads even while the bandwidth profile in force pauses downloads (e.g. one urgent download during the day). It is never faster than the global, profile or hand-set limits; those always apply. remove: true removes the package's own window, so it follows its category's. Refused with download_window.window_invalid or download_window.too_many_windows, package.not_found for an unknown id."
    )]
    pub async fn set_package_download_window(
        &self,
        Parameters(params): Parameters<DownloadWindowParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(params.request())?;
            let Json(stored) = packages::set_package_download_window(
                State(self.state.clone()),
                Path(id),
                Json(request),
            )
            .await?;
            Ok(stored)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Set the download window of a category's packages (id from list_configuration, section categories, which shows the current one): the same windows and ignore_schedule_pause as set_package_download_window, the default for every package of the category that has no window of its own. remove: true removes it, so the packages follow the bandwidth schedule alone. Refused with download_window.window_invalid or download_window.too_many_windows, category.not_found for an unknown id."
    )]
    pub async fn set_category_download_window(
        &self,
        Parameters(params): Parameters<DownloadWindowParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(params.request())?;
            let Json(stored) = config_handlers::set_category_download_window(
                State(self.state.clone()),
                axum::extract::Path(id),
                Json(request),
            )
            .await?;
            Ok(stored)
        }
        .await;
        respond(result)
    }
}
