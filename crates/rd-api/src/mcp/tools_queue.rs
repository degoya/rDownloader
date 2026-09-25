//! MCP tools for handling queued work: order, names, targets, tidying and unpacking
//! (RD-120-32).
//!
//! RD-120-29 left these out as positions in a list the caller cannot see, as renames a model
//! would only echo, and as repairs a person makes while watching. `list_downloads` and
//! `list_packages` show the list, with its ids; the rest was the premise, not a reason. Each
//! tool calls its route's handler, so the checks against running work — a rename of a file
//! still being written, a clear that would touch a package with a running member — are the
//! route's and nobody else's.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_config::IdParams,
    params_delivery::DefinitionParams,
    params_handling::{
        ClearPackagesParams, ClearScopeParam, ExtractPackagesParams, IdBodyParams, IdsParams,
        RenameParams, ReorderMembersParams, ReorderPackagesParams, body, public,
    },
};
use crate::{
    ApiError, download_handlers as downloads, error_codes::parse_id, package_handlers as packages,
};

#[tool_router(router = queue_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Set the order of the download packages in the queue: `ids` from list_packages, in their new order. Packages not named keep their relative order behind them."
    )]
    pub async fn reorder_packages(
        &self,
        Parameters(params): Parameters<ReorderPackagesParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({ "ids": params.ids }))?;
            let Json(answer) =
                packages::reorder_packages(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Set the order of the files inside one download package. `ids` must be exactly that package's downloads (list_downloads), each once, in the new order."
    )]
    pub async fn reorder_downloads(
        &self,
        Parameters(params): Parameters<ReorderMembersParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "package_id": params.package_id,
                "ids": params.ids,
            }))?;
            let Json(answer) =
                packages::reorder_downloads(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Rename one queued download (id from list_downloads): the file name it is written under. Refused while the file is being written."
    )]
    pub async fn rename_download(
        &self,
        Parameters(params): Parameters<RenameParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({ "file_name": params.name }))?;
            let Json(download) = downloads::rename_download(
                State(self.state.clone()),
                Path(params.id),
                Json(request),
            )
            .await?;
            Ok(download)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one download package (id from list_packages). `body` is the REST body of PATCH /api/v1/packages/{id}: name, category_id or clear_category, priority (low|normal|high), postprocess_level (none|repair|unpack|delete) or clear_postprocess_level, script or clear_script, clear_password. A new category moves the files to its folder."
    )]
    pub async fn update_package(
        &self,
        Parameters(params): Parameters<IdBodyParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::Value::Object(params.body))?;
            let Json(package) =
                packages::update_package(State(self.state.clone()), Path(id), Json(request))
                    .await?;
            public(&package)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change several download packages at once. `definition` is the REST body of POST /api/v1/packages/bulk: ids, plus category_id or clear_category, priority, postprocess_level or clear_postprocess_level, script or clear_script."
    )]
    pub async fn update_packages(
        &self,
        Parameters(params): Parameters<DefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::Value::Object(params.definition))?;
            let Json(changed) =
                packages::bulk_update_packages(State(self.state.clone()), Json(request)).await?;
            public(&changed)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Rename the folder a download package is written into, on disk and in the queue. Refused while a member is being written."
    )]
    pub async fn rename_package_folder(
        &self,
        Parameters(params): Parameters<RenameParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::json!({ "name": params.name }))?;
            let Json(package) =
                packages::rename_package_folder(State(self.state.clone()), Path(id), Json(request))
                    .await?;
            public(&package)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Remove finished packages from the download list in one sweep: scope completed (every file succeeded), failed (holds a failed or blocked file and has nothing left to do) or all (nothing is working any more). A package with a running, waiting or seeding member is never touched; the answer lists what was removed and what was skipped, with a code for why."
    )]
    pub async fn clear_finished_packages(
        &self,
        Parameters(params): Parameters<ClearPackagesParams>,
    ) -> McpToolResult {
        let result = async {
            let scope = match params.scope {
                ClearScopeParam::Completed => "completed",
                ClearScopeParam::Failed => "failed",
                ClearScopeParam::All => "all",
            };
            let request = body(serde_json::json!({ "scope": scope }))?;
            let Json(answer) =
                crate::package_clear::clear_packages(State(self.state.clone()), Json(request))
                    .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Unpack downloaded archive files now (ids from list_downloads), outside the post-processing policy."
    )]
    pub async fn extract_downloads(
        &self,
        Parameters(params): Parameters<IdsParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({ "ids": params.ids }))?;
            let (_, Json(answer)) =
                downloads::extract_downloads(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Unpack download packages now (ids from list_packages). force=true post-processes a package whose verification failed, once, anyway — the one-off answer to a broken PAR2 set beside intact archives; each package is then queued on its own."
    )]
    pub async fn extract_packages(
        &self,
        Parameters(params): Parameters<ExtractPackagesParams>,
    ) -> McpToolResult {
        let result = async {
            if params.force.unwrap_or(false) {
                let mut answers = Vec::new();
                for id in &params.ids {
                    let id = parse_id(id)?;
                    let (_, Json(answer)) =
                        packages::force_extract_package(State(self.state.clone()), Path(id))
                            .await?;
                    answers.push(answer);
                }
                return serde_json::to_value(answers).map_err(|error| {
                    ApiError::bad_request("request.body_invalid", error.to_string())
                });
            }
            let request = body(serde_json::json!({ "ids": params.ids }))?;
            let (_, Json(answer)) =
                packages::extract_packages(State(self.state.clone()), Json(request)).await?;
            serde_json::to_value(answer)
                .map_err(|error| ApiError::bad_request("request.body_invalid", error.to_string()))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Read the post-processing steps of one download package (id from list_packages): repair, unpack, scripts and uploads, with their state and output."
    )]
    pub async fn get_package_postprocess(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(steps) =
                packages::list_package_postprocess(State(self.state.clone()), Path(id)).await?;
            Ok(steps)
        }
        .await;
        respond(result)
    }
}
