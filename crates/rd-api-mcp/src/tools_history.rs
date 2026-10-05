//! MCP tools for the download history (RD-1100-04): search it, add an entry again, empty it.
//!
//! The answers carry no credential: the history keeps its sources masked and never an archive
//! password, so what a caller reads here is what the History view shows.

use axum::extract::{Path as AxumPath, State};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_insight::DataClearToolParams,
};
use rd_api_intake::history_readd_handlers;
use rd_api_queue::history_handlers::{self, HistoryFilter, HistoryFilterQuery};

use crate::{ApiError, data_reset_handlers::DataClearRequest, dto::PageQuery};

/// What `list_download_history` filters and pages by; every field is optional.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct HistoryListParams {
    /// Part of the name or of a source address, case-insensitive.
    #[serde(default)]
    pub query: Option<String>,
    /// `completed` or `failed`.
    #[serde(default)]
    pub outcome: Option<String>,
    /// The download kind: http, usenet, media, gallery, record, torrent, ftp, sftp, plugin or
    /// object_storage.
    #[serde(default)]
    pub kind: Option<String>,
    /// Entries that ended at or after this instant (RFC 3339).
    #[serde(default)]
    pub from: Option<String>,
    /// Entries that ended at or before this instant (RFC 3339).
    #[serde(default)]
    pub to: Option<String>,
    /// Entries to return, 1 to 1000 (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
    /// Entries to skip first (default 0).
    #[serde(default)]
    pub offset: Option<u32>,
}

/// The entry `readd_history_entry` adds again.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct HistoryEntryParams {
    /// The entry's `id` from `list_download_history`.
    pub id: i64,
}

#[derive(Serialize)]
struct HistoryListResult {
    /// How many entries the filters match in all.
    total: u64,
    entries: Vec<rd_core::HistoryEntry>,
}

/// A filter word read the way the REST query string reads it.
fn word<T: serde::de::DeserializeOwned>(value: Option<String>) -> Result<Option<T>, ApiError> {
    value
        .map(|value| {
            serde_json::from_value(serde_json::Value::String(value.trim().to_owned()))
                .map_err(|_| history_handlers::history_filter_invalid())
        })
        .transpose()
}

fn instant(value: Option<String>) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    value
        .map(|value| {
            chrono::DateTime::parse_from_rfc3339(value.trim())
                .map(|parsed| parsed.with_timezone(&chrono::Utc))
                .map_err(|_| history_handlers::history_filter_invalid())
        })
        .transpose()
}

#[tool_router(router = history_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Search the download history, newest first: every package that completed or failed, also after it was removed from the queue or cleaned up automatically. Each entry has the name, kind, category, destination, size, file count, the source addresses (credentials masked), the outcome and, for a failure, its stable error code. Filter by `query` (part of the name or a source), `outcome`, `kind` and the `from`/`to` time range; page with `limit` and `offset`. `total` counts every match."
    )]
    pub async fn list_download_history(
        &self,
        Parameters(params): Parameters<HistoryListParams>,
    ) -> McpToolResult {
        let result: Result<HistoryListResult, ApiError> = async {
            let filter = HistoryFilterQuery {
                q: params.query,
                outcome: word(params.outcome)?,
                kind: word(params.kind)?,
                from: instant(params.from)?,
                to: instant(params.to)?,
            };
            let (headers, axum::Json(entries)) = history_handlers::list_download_history(
                State(self.state.clone()),
                rd_api_core::list_bounds::Page(PageQuery {
                    limit: Some(params.limit.unwrap_or(50)),
                    offset: params.offset,
                }),
                HistoryFilter(filter),
            )
            .await?;
            let total = headers
                .get(rd_api_core::list_bounds::TOTAL_COUNT_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok())
                .unwrap_or_default();
            Ok(HistoryListResult { total, entries })
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Add a download history entry again: its source addresses go back into the LinkGrabber as one package under the entry's name, checked online like pasted links. Follow up with check_links and enqueue_collector. An entry without a source of its own (an imported NZB) is refused with history.nothing_to_readd."
    )]
    pub async fn readd_history_entry(
        &self,
        Parameters(params): Parameters<HistoryEntryParams>,
    ) -> McpToolResult {
        respond(
            history_readd_handlers::readd_history_entry(
                State(self.state.clone()),
                AxumPath(params.id),
            )
            .await
            .map(|(_, axum::Json(response))| response),
        )
    }

    #[tool(
        description = "Empty the download history. Irreversible, and `confirmed` must be true. Only the history's entries go: the queue, the files, the statistics and the logs are untouched."
    )]
    pub async fn clear_download_history(
        &self,
        Parameters(params): Parameters<DataClearToolParams>,
    ) -> McpToolResult {
        respond(
            crate::data_reset_handlers::clear_download_history(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                axum::Json(DataClearRequest {
                    confirmed: params.confirmed,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }
}
