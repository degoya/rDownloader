//! MCP tools for storage roots and watched folders.
//!
//! Same bargain as [`super::tools_routing`]: the tool assembles the REST request body and the
//! REST handler validates it, so the directory probe, the path allowlist and the error codes
//! are the ones the web UI gets.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_config::{
        CreateHotfolderParams, CreateStorageRootParams, IdParams, UpdateHotfolderParams,
        UpdateStorageRootParams, clearing, merged,
    },
};
use crate::{
    ApiError,
    dto::{CreateHotFolderRequest, CreateStorageRootRequest},
};

fn byte_count(value: u64) -> Result<rd_core::ByteCount, ApiError> {
    rd_core::ByteCount::new(value)
        .map_err(|error| ApiError::bad_request("request.byte_count_invalid", error))
}

#[tool_router(router = storage_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Create a storage root: an absolute directory downloads may be written into. The directory is created and probed for writability."
    )]
    pub async fn create_storage_root(
        &self,
        Parameters(params): Parameters<CreateStorageRootParams>,
    ) -> McpToolResult {
        let result = async {
            let request = CreateStorageRootRequest {
                name: params.name,
                path: params.path,
                is_default: params.is_default.unwrap_or(false),
                minimum_free_bytes: params.minimum_free_bytes.map(byte_count).transpose()?,
            };
            Ok(crate::config_handlers::create_storage_root(
                State(self.state.clone()),
                Json(request),
            )
            .await?
            .1
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one storage root. Only the fields you pass are changed; `clear` may reset minimum_free_bytes to the global reserve."
    )]
    pub async fn update_storage_root(
        &self,
        Parameters(params): Parameters<UpdateStorageRootParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::StorageRootId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_storage_roots()
                .await?
                .into_iter()
                .find(|root| root.id == id)
                .ok_or_else(|| {
                    ApiError::not_found("storage_root.not_found", "Storage root not found")
                })?;
            let cleared = clearing(params.clear.as_ref(), &["minimum_free_bytes"])?;
            let request = CreateStorageRootRequest {
                name: params.name.unwrap_or(current.name),
                path: params.path.unwrap_or(current.path),
                is_default: params.is_default.unwrap_or(current.is_default),
                minimum_free_bytes: merged(
                    &cleared,
                    "minimum_free_bytes",
                    params.minimum_free_bytes.map(byte_count).transpose()?,
                    current.minimum_free_bytes,
                ),
            };
            Ok(crate::config_handlers::update_storage_root(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one storage root. Destructive and refused while categories still point at it; the directory itself stays on disk."
    )]
    pub async fn delete_storage_root(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::StorageRootId = parse_id(&params.id)?;
            Ok(crate::config_handlers::delete_storage_root(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                AxumPath(id),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the watched folders that import .nzb, .dlc and link files dropped into them."
    )]
    pub async fn list_hotfolders(&self) -> McpToolResult {
        respond(
            crate::config_handlers::list_hotfolders(State(self.state.clone()))
                .await
                .map(|folders| folders.0),
        )
    }

    #[tool(
        description = "Create a watched folder. `import_mode` decides whether its files wait in the LinkGrabber or go straight into the queue."
    )]
    pub async fn create_hotfolder(
        &self,
        Parameters(params): Parameters<CreateHotfolderParams>,
    ) -> McpToolResult {
        let result = async {
            let request = CreateHotFolderRequest {
                name: params.name,
                executor: match params.capture_agent_id.as_deref() {
                    Some(agent) => rd_core::HotFolderExecutor::CaptureAgent {
                        agent_id: parse_id(agent)?,
                    },
                    None => rd_core::HotFolderExecutor::Daemon,
                },
                path: params.path,
                recursive: params.recursive.unwrap_or(false),
                category_id: params.category_id.as_deref().map(parse_id).transpose()?,
                import_mode: params.import_mode.into(),
                processed_path: params.processed_path,
                failed_path: params.failed_path,
                enabled: params.enabled.unwrap_or(true),
            };
            Ok(
                crate::config_handlers::create_hotfolder(State(self.state.clone()), Json(request))
                    .await?
                    .1
                    .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(description = "Change one watched folder. Only the fields you pass are changed.")]
    pub async fn update_hotfolder(
        &self,
        Parameters(params): Parameters<UpdateHotfolderParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::HotFolderId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_hotfolders()
                .await?
                .into_iter()
                .find(|folder| folder.id == id)
                .ok_or_else(|| ApiError::not_found("hotfolder.not_found", "Hotfolder not found"))?;
            let cleared = clearing(params.clear.as_ref(), &["category_id", "capture_agent_id"])?;
            let current_agent = match current.executor {
                rd_core::HotFolderExecutor::CaptureAgent { agent_id } => Some(agent_id.to_string()),
                rd_core::HotFolderExecutor::Daemon => None,
            };
            let agent = merged(
                &cleared,
                "capture_agent_id",
                params.capture_agent_id,
                current_agent,
            );
            let request = CreateHotFolderRequest {
                name: params.name.unwrap_or(current.name),
                executor: match agent.as_deref() {
                    Some(agent) => rd_core::HotFolderExecutor::CaptureAgent {
                        agent_id: parse_id(agent)?,
                    },
                    None => rd_core::HotFolderExecutor::Daemon,
                },
                path: params.path.unwrap_or(current.path),
                recursive: params.recursive.unwrap_or(current.recursive),
                category_id: merged(
                    &cleared,
                    "category_id",
                    params.category_id.as_deref().map(parse_id).transpose()?,
                    current.category_id,
                ),
                import_mode: params.import_mode.map_or(current.import_mode, Into::into),
                processed_path: params.processed_path.unwrap_or(current.processed_path),
                failed_path: params.failed_path.unwrap_or(current.failed_path),
                enabled: params.enabled.unwrap_or(current.enabled),
            };
            Ok(crate::config_handlers::update_hotfolder(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one watched folder. Destructive; the folder and its files stay on disk, they are simply no longer watched."
    )]
    pub async fn delete_hotfolder(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::HotFolderId = parse_id(&params.id)?;
            Ok(
                crate::config_handlers::delete_hotfolder(State(self.state.clone()), AxumPath(id))
                    .await?
                    .0,
            )
        }
        .await;
        respond(result)
    }
}
