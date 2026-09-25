//! MCP tools operating on the download list and packages.

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use serde::Serialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, json_result, parse_id, parse_ids, respond},
    params::{
        AddDownloadsParams, ControlDownloadsParams, DeletePackagesParams, DownloadItem,
        GetDownloadParams, ListDownloadsParams, ListPackagesParams, PackageItem, paginate,
    },
};
use crate::dto::CreateDownloadRequest;

#[derive(Serialize)]
struct AddedDownload {
    url: String,
    download: DownloadItem,
}

#[derive(Serialize)]
struct FailedDownload {
    url: String,
    error: String,
    code: String,
}

#[derive(Serialize)]
struct AddDownloadsResult {
    created: Vec<AddedDownload>,
    failed: Vec<FailedDownload>,
}

#[tool_router(router = downloads_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Add direct HTTP(S) or magnet downloads to the queue. For hoster/one-click links (rapidgator, keep2share, ...) use collect_links + enqueue_collector instead, so accounts and link checks apply."
    )]
    pub async fn add_downloads(
        &self,
        Parameters(params): Parameters<AddDownloadsParams>,
    ) -> McpToolResult {
        if params.urls.is_empty() || params.urls.len() > 50 {
            return Ok(api_error(crate::error_codes::bulk_range(50)));
        }
        let category_id = match params.category_id.as_deref().map(parse_id).transpose() {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let account_id = match params.account_id.as_deref().map(parse_id).transpose() {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let start_paused = params.start_paused.unwrap_or(false);
        let mut created = Vec::new();
        let mut failed = Vec::new();
        for url in params.urls {
            let request = CreateDownloadRequest {
                url: url.clone(),
                package_name: params.package_name.clone(),
                file_name: None,
                category_id,
                account_id,
                proxy_profile_id: None,
                priority: params.priority.map(Into::into),
            };
            match crate::download_handlers::create_download_inner(&self.state, request).await {
                Ok(file) => {
                    if start_paused {
                        let _ = self.state.scheduler.pause(file.id).await;
                    }
                    created.push(AddedDownload {
                        url,
                        download: file.into(),
                    });
                }
                Err(error) => failed.push(FailedDownload {
                    url,
                    error: error.message().to_owned(),
                    code: error.code().to_owned(),
                }),
            }
        }
        json_result(&AddDownloadsResult { created, failed })
    }

    #[tool(
        description = "List downloads with optional state/package/name filters, paginated. Poll this or get_status_summary to observe progress."
    )]
    pub async fn list_downloads(
        &self,
        Parameters(params): Parameters<ListDownloadsParams>,
    ) -> McpToolResult {
        let package_id = match params.package_id.as_deref().map(parse_id).transpose() {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let result = async {
            let mut rows = self.state.database.list_downloads().await?;
            if let Some(filter) = params.state {
                rows.retain(|file| filter.matches(file.state));
            }
            if let Some(package_id) = package_id {
                rows.retain(|file: &rd_core::DownloadFile| file.package_id == package_id);
            }
            if let Some(needle) = params
                .name_contains
                .as_deref()
                .map(str::to_lowercase)
                .filter(|needle| !needle.is_empty())
            {
                rows.retain(|file| file.file_name.to_lowercase().contains(&needle));
            }
            Ok(paginate(
                rows,
                params.limit,
                params.offset,
                DownloadItem::from,
            ))
        }
        .await;
        respond(result)
    }

    #[tool(description = "Full detail of one download file, including source URL and checksums.")]
    pub async fn get_download(
        &self,
        Parameters(params): Parameters<GetDownloadParams>,
    ) -> McpToolResult {
        let id = match parse_id::<rd_core::DownloadId>(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let result = async {
            self.state
                .database
                .get_download(id)
                .await?
                .ok_or_else(crate::error_codes::download_not_found)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Pause, resume, cancel or remove downloads by id (bulk, 1-500 ids). remove deletes the list entry; active files are cancelled first."
    )]
    pub async fn control_downloads(
        &self,
        Parameters(params): Parameters<ControlDownloadsParams>,
    ) -> McpToolResult {
        let ids = match parse_ids(&params.ids) {
            Ok(ids) => ids,
            Err(error) => return Ok(api_error(error)),
        };
        respond(
            crate::download_handlers::apply_download_action(&self.state, params.action.into(), ids)
                .await,
        )
    }

    #[tool(
        description = "Queue overview: per-state counts, byte totals, remaining bytes, the queue's current transfer rate and the seconds it still needs at that rate, and free storage space. The rate and the remaining time are absent when no honest figure exists."
    )]
    pub async fn get_status_summary(&self) -> McpToolResult {
        respond(crate::download_handlers::summarize_downloads(&self.state).await)
    }

    #[tool(description = "List download packages (folders grouping files), paginated.")]
    pub async fn list_packages(
        &self,
        Parameters(params): Parameters<ListPackagesParams>,
    ) -> McpToolResult {
        let result = async {
            let rows = self.state.database.list_packages().await?;
            Ok(paginate(
                rows,
                params.limit,
                params.offset,
                PackageItem::from,
            ))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Remove whole packages including every file entry (1-500 ids). A package that is still running, waiting, seeding or being post-processed is refused unless force is set, in which case its files are cancelled and their incomplete data deleted."
    )]
    pub async fn delete_packages(
        &self,
        Parameters(params): Parameters<DeletePackagesParams>,
    ) -> McpToolResult {
        let ids = match parse_ids(&params.ids) {
            Ok(ids) => ids,
            Err(error) => return Ok(api_error(error)),
        };
        if ids.is_empty() || ids.len() > 500 {
            return Ok(api_error(crate::error_codes::bulk_range(500)));
        }
        let result = async {
            let removed = crate::package_handlers::remove_packages(
                &self.state,
                ids,
                params.force.unwrap_or(false),
            )
            .await?;
            Ok(serde_json::json!({ "removed_packages": removed }))
        }
        .await;
        respond(result)
    }
}
