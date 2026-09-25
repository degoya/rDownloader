//! MCP tools for the LinkGrabber's package level and the NZB review (RD-120-32).
//!
//! Package editing and ordering were out under RD-120-29 as drag-and-drop; the NZB review was
//! out because a tool could hand an NZB in but nothing could queue it. Both now call their
//! route's handler: the package tools with the REST body built from their arguments, the NZB
//! tools with the import id `list_nzb_imports` hands out.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, parse_id, respond},
    params_config::IdParams,
    params_handling::{
        EnqueueNzbParams, EntryKindParam, EntryRefParam, IdBodyParams, ReorderCollectorParams,
        body, public,
    },
};
use crate::{ApiError, collector_handlers as collector, handlers};

fn entry(reference: &EntryRefParam) -> serde_json::Value {
    serde_json::json!({
        "kind": match reference.kind {
            EntryKindParam::Collector => "collector",
            EntryKindParam::Nzb => "nzb",
        },
        "id": reference.id,
    })
}

#[tool_router(router = grabber_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Change one LinkGrabber package before it is enqueued (id from list_collector). `body` is the REST body of PATCH /api/v1/collector/packages/{id}: name, category_id or clear_category, priority (low|normal|high), postprocess_level (none|repair|unpack|delete) or clear_postprocess_level, script or clear_script, clear_password. An archive password is set with collect_links, not here."
    )]
    pub async fn update_collector_package(
        &self,
        Parameters(params): Parameters<IdBodyParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::Value::Object(params.body))?;
            let Json(package) = collector::update_collector_package(
                State(self.state.clone()),
                Path(id),
                Json(request),
            )
            .await?;
            public(&package)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change several LinkGrabber packages at once. `definition` is the REST body of POST /api/v1/collector/packages/bulk: ids, plus category_id or clear_category, priority, postprocess_level or clear_postprocess_level, script or clear_script."
    )]
    pub async fn update_collector_packages(
        &self,
        Parameters(params): Parameters<super::params_delivery::DefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::Value::Object(params.definition))?;
            let Json(packages) =
                collector::bulk_update_collector_packages(State(self.state.clone()), Json(request))
                    .await?;
            public(&packages)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Remove one LinkGrabber package and its links (id from list_collector). A package being checked or enqueued is refused with collector.package_busy."
    )]
    pub async fn delete_collector_package(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(answer) =
                collector::delete_collector_package(State(self.state.clone()), Path(id)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Regroup every LinkGrabber batch from scratch: links are sorted into packages again by the grouping rules, as the LinkGrabber's regroup button does."
    )]
    pub async fn regroup_collector(&self) -> McpToolResult {
        respond(
            collector::regroup_collector_packages(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Move LinkGrabber entries — packages (kind collector, id from list_collector) and NZB imports (kind nzb, id from list_nzb_imports) — to a new place in the list: `entries` in their new order, placed behind `after`, or at the top when `after` is absent."
    )]
    pub async fn reorder_collector(
        &self,
        Parameters(params): Parameters<ReorderCollectorParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "entries": params.entries.iter().map(entry).collect::<Vec<_>>(),
                "after": params.after.as_ref().map(entry),
            }))?;
            let Json(answer) =
                collector::reorder_grabber_entries(State(self.state.clone()), Json(request))
                    .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the NZB imports waiting in the LinkGrabber for review: name, size, file count, category, priority and state, with the ids the other nzb_import tools take."
    )]
    pub async fn list_nzb_imports(&self) -> McpToolResult {
        respond(
            handlers::list_nzb_imports(State(self.state.clone()))
                .await
                .and_then(|Json(imports)| public(&imports)),
        )
    }

    #[tool(
        description = "Read one NZB import (id from list_nzb_imports). view=files: its files with their segment state; view=postprocess: the post-processing steps planned or run for it."
    )]
    pub async fn get_nzb_import(
        &self,
        Parameters(params): Parameters<super::params_handling::NzbViewParams>,
    ) -> McpToolResult {
        let id: rd_core::NzbImportId = match parse_id(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let state = State(self.state.clone());
        match params.view {
            super::params_handling::NzbView::Files => respond(
                crate::usenet_handlers::list_nzb_files(state, Path(id))
                    .await
                    .map(|Json(answer)| answer),
            ),
            super::params_handling::NzbView::Postprocess => respond(
                crate::usenet_handlers::list_postprocess_steps(state, Path(id))
                    .await
                    .map(|Json(answer)| answer),
            ),
        }
    }

    #[tool(
        description = "Change an NZB import before it is queued. `body` is the REST body of PATCH /api/v1/nzb/imports/{id}: category_id or clear_category, priority (low|normal|high)."
    )]
    pub async fn update_nzb_import(
        &self,
        Parameters(params): Parameters<IdBodyParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::Value::Object(params.body))?;
            let Json(import) =
                handlers::update_nzb_import(State(self.state.clone()), Path(id), Json(request))
                    .await?;
            public(&import)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Queue a reviewed NZB import: it becomes a download package and starts, or waits paused. Answers with the package."
    )]
    pub async fn enqueue_nzb_import(
        &self,
        Parameters(params): Parameters<EnqueueNzbParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request: crate::dto::NzbImportEnqueueRequest =
                body(serde_json::json!({ "paused": params.paused.unwrap_or(false) }))?;
            let (_, Json(package)) = crate::usenet_handlers::enqueue_nzb_import(
                State(self.state.clone()),
                Path(id),
                Some(Json(request)),
            )
            .await?;
            public(&package)
        }
        .await;
        respond(result)
    }

    #[tool(description = "Discard an NZB import without queueing it.")]
    pub async fn delete_nzb_import(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(answer) =
                handlers::delete_nzb_import(State(self.state.clone()), Path(id)).await?;
            Ok::<_, ApiError>(answer)
        }
        .await;
        respond(result)
    }
}
