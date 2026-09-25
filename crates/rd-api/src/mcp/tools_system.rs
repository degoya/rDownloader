//! MCP tools for post-processing, the managed external tools and storage capacity
//! (RD-120-32).
//!
//! The post-processing lists were out because they are dropdowns, and the reason given was that
//! a status token must not enumerate the installation. That reason is about the *price*, and
//! the price is `scope_policy`'s: these tools cost `api:queue` exactly as their routes do, so a
//! status token still cannot read them. Managed tools and storage capacity were out as
//! administration and as "revisit if the code on the row proves too thin"; both are now one
//! call to the route that answers the screen.
//!
//! `get_about` reads the About page's head (RD-130-12). Not its licence list: a thousand
//! entries answer no question an agent is asked, and the route stays one call away for a person.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_handling::{
        IdBodyParams, ManageAction, ManageToolParams, ManagedToolsParams, ManagedToolsView,
        PostprocessOptions, PostprocessOptionsParams, StorageTargetParams, body,
    },
};
use crate::{ApiError, postprocess_handlers as postprocess, tools_handlers as tools};

fn value<T: serde::Serialize>(answer: T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(answer)
        .map_err(|error| ApiError::bad_request("request.body_invalid", error.to_string()))
}

#[tool_router(router = system_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List what post-processing can be set to. kind=scripts: the user scripts a category or package may run; plugin_steps: the steps installed plugins provide; upload_destinations: the upload targets installed plugins provide. The names are what update_category_postprocess, update_package and update_collector_package take."
    )]
    pub async fn list_postprocess_options(
        &self,
        Parameters(params): Parameters<PostprocessOptionsParams>,
    ) -> McpToolResult {
        let state = State(self.state.clone());
        let result = async {
            match params.kind {
                PostprocessOptions::Scripts => {
                    value(postprocess::list_postprocess_scripts(state).await?.0)
                }
                PostprocessOptions::PluginSteps => {
                    value(postprocess::list_plugin_steps(state).await.0)
                }
                PostprocessOptions::UploadDestinations => {
                    value(postprocess::list_upload_destinations(state).await.0)
                }
            }
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the post-processing queue: packages waiting to be repaired, unpacked or handed to a script, and the one running now."
    )]
    pub async fn list_postprocess_queue(&self) -> McpToolResult {
        respond(
            postprocess::list_postprocess_queue(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Change only the post-processing of one category (id from list_configuration section categories). `body` is the REST body of PATCH /api/v1/categories/{id}/postprocess: postprocess_level, script, cleanup_extensions, recursive_unpack, sfv_verify, safe_postproc, delete_par2, plugin_steps, upload_enabled, upload_remote. Names come from list_postprocess_options."
    )]
    pub async fn update_category_postprocess(
        &self,
        Parameters(params): Parameters<IdBodyParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::Value::Object(params.body))?;
            let Json(category) = postprocess::update_category_postprocess(
                State(self.state.clone()),
                Path(id),
                Json(request),
            )
            .await?;
            Ok(category)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Read the managed external tools. view=tools (default): each tool (yt-dlp, ffmpeg, ...), its installed versions, the active one and what the signed manifest offers; view=media: whether yt-dlp and ffmpeg are usable right now and from where."
    )]
    pub async fn list_managed_tools(
        &self,
        Parameters(params): Parameters<ManagedToolsParams>,
    ) -> McpToolResult {
        let state = State(self.state.clone());
        let result = async {
            match params.view.unwrap_or(ManagedToolsView::Tools) {
                ManagedToolsView::Tools => value(tools::list_managed_tools(state).await?.0),
                ManagedToolsView::Media => value(crate::handlers::media_status(state).await?.0),
            }
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change a managed external tool (name from list_managed_tools). action=install downloads and verifies a version against the signed manifest; activate makes an installed version the one in use; rollback returns to the version active before. `version` is for install and activate; absent means the manifest's current one."
    )]
    pub async fn manage_tool(
        &self,
        Parameters(params): Parameters<ManageToolParams>,
    ) -> McpToolResult {
        let result = async {
            let state = State(self.state.clone());
            let name = Path(params.name);
            let Json(answer) = match params.action {
                ManageAction::Install => {
                    let request = body(serde_json::json!({ "version": params.version }))?;
                    tools::install_managed_tool(state, name, Json(request)).await?
                }
                ManageAction::Activate => {
                    let request = body(serde_json::json!({ "version": params.version }))?;
                    tools::activate_managed_tool(state, name, Json(request)).await?
                }
                ManageAction::Rollback => tools::rollback_managed_tool(state, name).await?,
            };
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Fetch the signed tool manifest again, so list_managed_tools shows what is newly available."
    )]
    pub async fn refresh_tool_manifest(&self) -> McpToolResult {
        respond(
            tools::refresh_tool_manifest(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read free space per storage target: each storage root and the fallback folder, its free and reserved bytes, and whether the service has paused work on it for lack of space."
    )]
    pub async fn get_storage_capacity(&self) -> McpToolResult {
        respond(
            crate::storage_capacity::storage_capacity(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Resume work on a storage target the service paused for lack of space, once space has been made. `target` is a storage root id from get_storage_capacity, or `fallback`."
    )]
    pub async fn resume_storage_target(
        &self,
        Parameters(params): Parameters<StorageTargetParams>,
    ) -> McpToolResult {
        respond(
            crate::storage_capacity::resume_storage(State(self.state.clone()), Path(params.target))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Which rDownloader is running: version, commit and build time, the plugin contract versions it links, the platform, its licence and authors, the project's addresses (each says whether it is published yet), and the helper tools the packages ship with their licences."
    )]
    pub async fn get_about(&self) -> McpToolResult {
        let Json(answer) = crate::about::system_about(State(self.state.clone())).await;
        respond(Ok::<_, ApiError>(answer))
    }
}
