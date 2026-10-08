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
        description = "Add direct HTTP(S) or magnet downloads to the queue. With start_paused they are created paused -- the scheduler never starts them -- and wait for control_downloads resume. For hoster/one-click links (rapidgator, keep2share, ...) use collect_links + enqueue_collector instead, so accounts and link checks apply. An address an agent hands in never reaches this machine or the local network: a literal loopback, private or link-local address fails with mirror.internal_address, and a name that resolves there fails the same way when the transfer starts."
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
                // Written paused in the same row write; pausing afterwards left the scheduler
                // a moment to start the file first (API-09).
                paused: start_paused,
            };
            match crate::download_handlers::create_download_as(&self.state, request, Some(false))
                .await
            {
                Ok(file) => created.push(AddedDownload {
                    url,
                    download: file.into(),
                }),
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
        description = "List downloads with optional state/package/name filters, paginated. name_contains matches the file name or the package name, like the web UI's search. Poll this or get_status_summary to observe progress. A failed row's error says why; get_download has its stable code and params. A queued row with waiting_for_host is held back because that host has no free connection (per-host connection limit, max_connections_per_host); it takes no parallel-download place meanwhile, and a file of another host starts instead."
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
                let packages: std::collections::HashSet<rd_core::PackageId> = self
                    .state
                    .database
                    .list_packages()
                    .await?
                    .into_iter()
                    .filter(|package| package.name.to_lowercase().contains(&needle))
                    .map(|package| package.id)
                    .collect();
                rows.retain(|file| {
                    packages.contains(&file.package_id)
                        || file.file_name.to_lowercase().contains(&needle)
                });
            }
            let host_waits = self.state.scheduler.host_waits().await;
            Ok(paginate(rows, params.limit, params.offset, |file| {
                // The last dispatch pass's word, for a row that is still queued.
                let waiting_for_host = (file.state == rd_core::DownloadState::Queued)
                    .then(|| host_waits.get(&file.id).cloned())
                    .flatten();
                DownloadItem {
                    waiting_for_host,
                    ..DownloadItem::from(file)
                }
            }))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Full detail of one download file, including source URL and checksums. last_error carries a failure's stable code and params - e.g. usenet.job_hopeless with missing_blocks and available_blocks for a Usenet download given up as beyond repair (switched by fail_hopeless_jobs in the settings)."
    )]
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
        description = "The mirrors of one download that came from a Metalink file, in the order they are tried: address (redacted), priority, location, whether each is ready, backing off or isolated after wrong bytes, and how many bytes each delivered. Empty for a download with a single address."
    )]
    pub async fn get_download_sources(
        &self,
        Parameters(params): Parameters<GetDownloadParams>,
    ) -> McpToolResult {
        let id = match parse_id::<rd_core::DownloadId>(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        respond(
            crate::download_sources::list_download_sources(
                axum::extract::State(self.state.clone()),
                axum::extract::Path(id),
            )
            .await
            .map(|axum::Json(sources)| sources),
        )
    }

    #[tool(
        description = "Pause, resume, cancel, remove or reset downloads, by id (bulk, 1-500 ids) or by state: with states instead of ids it acts on every download in those state groups, optionally only of package_id -- e.g. action reset, states [failed, blocked] starts every stuck file over. remove deletes the list entry; active files are cancelled first."
    )]
    pub async fn control_downloads(
        &self,
        Parameters(params): Parameters<ControlDownloadsParams>,
    ) -> McpToolResult {
        let ids = match parse_ids(&params.ids) {
            Ok(ids) => ids,
            Err(error) => return Ok(api_error(error)),
        };
        let filter = match control_filter(&params) {
            Ok(filter) => filter,
            Err(error) => return Ok(api_error(error)),
        };
        respond(
            crate::download_handlers::apply_download_action_to(
                &self.state,
                params.action.into(),
                ids,
                filter,
            )
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
        if let Err(error) = rd_api_core::list_bounds::validate_bulk(ids.len()) {
            return Ok(api_error(error));
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

/// The state filter of `control_downloads`, or `None` when it names its files by id.
fn control_filter(
    params: &ControlDownloadsParams,
) -> Result<Option<crate::dto::DownloadBulkFilter>, crate::ApiError> {
    if params.states.is_empty() && params.package_id.is_none() {
        return Ok(None);
    }
    let package_id = params.package_id.as_deref().map(parse_id).transpose()?;
    let mut states = Vec::new();
    for group in &params.states {
        states.extend_from_slice(group.states());
    }
    Ok(Some(crate::dto::DownloadBulkFilter { states, package_id }))
}
