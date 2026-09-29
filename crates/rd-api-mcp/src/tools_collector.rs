//! MCP tools for the LinkGrabber: intake, link checks, review and enqueue.

use std::time::Duration;

use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use serde::Serialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, json_result, parse_id, parse_ids, respond},
    params::{
        CandidateItem, CheckLinksParams, CollectLinksParams, CollectorPackageItem,
        EnqueueCollectorParams, ListCollectorParams, PackageItem, paginate,
    },
};
use crate::{ApiError, dto::CollectorIntakeRequest};

#[derive(Serialize)]
struct CollectLinksResult {
    batch_id: String,
    skipped_excluded: u32,
    packages: Vec<CollectorPackageItem>,
}

#[derive(Serialize)]
struct CheckLinksResult {
    /// `false` when the timeout elapsed while some links were still being checked.
    settled: bool,
    candidates: Vec<CandidateItem>,
}

#[derive(Serialize)]
struct EnqueueCollectorResult {
    enqueued: Vec<PackageItem>,
    errors: Vec<String>,
}

fn group_packages(
    packages: Vec<rd_core::CollectorPackage>,
    candidates: Vec<rd_core::LinkCandidate>,
) -> Vec<CollectorPackageItem> {
    packages
        .into_iter()
        .map(|package| {
            let members = candidates
                .iter()
                .filter(|candidate| candidate.package_id == Some(package.id))
                .cloned()
                .map(CandidateItem::from)
                .collect();
            CollectorPackageItem::new(package, members)
        })
        .collect()
}

fn is_settled(state: rd_core::LinkCandidateState) -> bool {
    !matches!(
        state,
        rd_core::LinkCandidateState::Checking | rd_core::LinkCandidateState::Resolving
    )
}

#[tool_router(router = collector_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Paste URLs or free text into the LinkGrabber. Links are grouped into packages and checked online automatically; follow up with check_links, then enqueue_collector to start downloading. This is the right entry point for hoster/one-click links."
    )]
    pub async fn collect_links(
        &self,
        Parameters(params): Parameters<CollectLinksParams>,
    ) -> McpToolResult {
        let request = CollectorIntakeRequest {
            text: Some(params.text),
            source: rd_core::IngressSource::Api,
            source_label: Some("mcp".to_owned()),
            package_name: params.package_name,
            password: params.password,
            links: Vec::new(),
        };
        let result = async {
            let response =
                crate::collector_handlers::collector_intake_inner(&self.state, request).await?;
            Ok(CollectLinksResult {
                batch_id: response.batch.id.to_string(),
                skipped_excluded: response.skipped_excluded,
                packages: group_packages(response.packages, response.candidates),
            })
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Trigger an online check for collector links and wait briefly for results. Pass candidate_ids or batch_id. Links still in checking state are returned as-is with settled=false; call again to poll."
    )]
    pub async fn check_links(
        &self,
        Parameters(params): Parameters<CheckLinksParams>,
    ) -> McpToolResult {
        let wait = Duration::from_secs(u64::from(params.wait_seconds.unwrap_or(10).min(30)));
        let result: Result<CheckLinksResult, ApiError> = async {
            let target_ids: Vec<rd_core::CandidateId> =
                match (&params.candidate_ids, &params.batch_id) {
                    (Some(ids), _) if !ids.is_empty() => {
                        let ids = parse_ids(ids)?;
                        self.state.link_check.check(ids.clone()).await;
                        ids
                    }
                    (_, Some(batch_id)) => {
                        let batch_id: rd_core::BatchId = parse_id(batch_id)?;
                        self.state.link_check.check_batch(batch_id).await;
                        self.state
                            .database
                            .list_candidates()
                            .await?
                            .into_iter()
                            .filter(|candidate| candidate.batch_id == batch_id)
                            .map(|candidate| candidate.id)
                            .collect()
                    }
                    _ => {
                        return Err(ApiError::bad_request(
                            "collector.check_target_missing",
                            "Provide candidate_ids or batch_id",
                        ));
                    }
                };
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let candidates: Vec<rd_core::LinkCandidate> = self
                    .state
                    .database
                    .list_candidates()
                    .await?
                    .into_iter()
                    .filter(|candidate| target_ids.contains(&candidate.id))
                    .collect();
                let settled = candidates
                    .iter()
                    .all(|candidate| is_settled(candidate.state) && candidate.checked_at.is_some());
                if settled || tokio::time::Instant::now() >= deadline {
                    return Ok(CheckLinksResult {
                        settled,
                        candidates: candidates.into_iter().map(CandidateItem::from).collect(),
                    });
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List LinkGrabber packages with their analyzed links (state, file name, size), paginated over packages."
    )]
    pub async fn list_collector(
        &self,
        Parameters(params): Parameters<ListCollectorParams>,
    ) -> McpToolResult {
        let batch_id = match params
            .batch_id
            .as_deref()
            .map(parse_id::<rd_core::BatchId>)
            .transpose()
        {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let result = async {
            let mut packages = self.state.database.list_collector_packages().await?;
            if let Some(batch_id) = batch_id {
                packages.retain(|package| package.batch_id == batch_id);
            }
            let candidates = self.state.database.list_candidates().await?;
            let page = paginate(packages, params.limit, params.offset, |package| package);
            Ok(super::params::PagedList {
                items: group_packages(page.items, candidates),
                total: page.total,
                truncated: page.truncated,
            })
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Move collector packages into the download list and start them (or create them paused). Returns the created download packages."
    )]
    pub async fn enqueue_collector(
        &self,
        Parameters(params): Parameters<EnqueueCollectorParams>,
    ) -> McpToolResult {
        let ids: Vec<rd_core::CollectorPackageId> = match parse_ids(&params.package_ids) {
            Ok(ids) => ids,
            Err(error) => return Ok(api_error(error)),
        };
        if ids.is_empty() || ids.len() > 500 {
            return Ok(api_error(crate::error_codes::bulk_range(500)));
        }
        let start_paused = params.start_paused.unwrap_or(false);
        let mut enqueued = Vec::new();
        let mut errors = Vec::new();
        for id in ids {
            match crate::collector_enqueue::enqueue_package(&self.state, id, start_paused, None)
                .await
            {
                Ok(outcome) => enqueued.push(PackageItem::from(outcome.package)),
                Err(error) => errors.push(format!("{id}: {}", error.message())),
            }
        }
        if enqueued.is_empty() && !errors.is_empty() {
            return Ok(api_error(ApiError::conflict(
                "collector.enqueue_failed",
                errors.join("; "),
            )));
        }
        json_result(&EnqueueCollectorResult { enqueued, errors })
    }
}
