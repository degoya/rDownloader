//! MCP tools for the routing configuration: download categories and their routing rules.
//!
//! Every tool builds the REST request body and hands it to the REST handler, so validation,
//! the storage-root allowlist and the stable error codes are the ones the web UI gets.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_config::{
        CreateCategoryParams, CreateCategoryRuleParams, IdParams, UpdateCategoryParams,
        UpdateCategoryRuleParams, clearing, merged,
    },
};
use crate::{
    ApiError, AppState,
    dto::{CreateCategoryRequest, CreateCategoryRuleRequest},
};

/// The category this id names, or the same 404 the REST layer returns.
async fn category(
    state: &AppState,
    id: rd_core::CategoryId,
) -> Result<rd_core::Category, ApiError> {
    state
        .database
        .list_categories()
        .await?
        .into_iter()
        .find(|category| category.id == id)
        .ok_or_else(|| ApiError::not_found("category.not_found", "Category not found"))
}

#[tool_router(router = routing_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Create a download category: a name, a colour, a storage root and a folder below it, plus optional post-processing defaults."
    )]
    pub async fn create_category(
        &self,
        Parameters(params): Parameters<CreateCategoryParams>,
    ) -> McpToolResult {
        let result = async {
            let request = CreateCategoryRequest {
                name: params.name,
                color: params.color,
                storage_root_id: parse_id(&params.storage_root_id)?,
                relative_path: params.relative_path,
                is_default: params.is_default.unwrap_or(false),
                postprocess_level: params.postprocess_level.map(Into::into),
                script: params.script,
                cleanup_extensions: params.cleanup_extensions,
                recursive_unpack: params.recursive_unpack,
                sfv_verify: params.sfv_verify,
                safe_postproc: params.safe_postproc,
                delete_par2: params.delete_par2,
                upload_enabled: params.upload_enabled,
                upload_remote: params.upload_remote,
            };
            Ok(
                crate::config_handlers::create_category(State(self.state.clone()), Json(request))
                    .await?
                    .1
                    .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one category. Only the fields you pass are changed; list nullable fields in `clear` to reset them to the global default."
    )]
    pub async fn update_category(
        &self,
        Parameters(params): Parameters<UpdateCategoryParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::CategoryId = parse_id(&params.id)?;
            let current = category(&self.state, id).await?;
            let cleared = clearing(
                params.clear.as_ref(),
                &[
                    "postprocess_level",
                    "script",
                    "cleanup_extensions",
                    "recursive_unpack",
                    "sfv_verify",
                    "safe_postproc",
                    "delete_par2",
                    "upload_enabled",
                    "upload_remote",
                ],
            )?;
            let request = CreateCategoryRequest {
                name: params.name.unwrap_or(current.name),
                color: params.color.unwrap_or(current.color),
                storage_root_id: match params.storage_root_id {
                    Some(value) => parse_id(&value)?,
                    None => current.storage_root_id,
                },
                relative_path: params.relative_path.unwrap_or(current.relative_path),
                is_default: params.is_default.unwrap_or(current.is_default),
                postprocess_level: merged(
                    &cleared,
                    "postprocess_level",
                    params.postprocess_level.map(Into::into),
                    current.postprocess_level,
                ),
                script: merged(&cleared, "script", params.script, current.script),
                cleanup_extensions: merged(
                    &cleared,
                    "cleanup_extensions",
                    params.cleanup_extensions,
                    current.cleanup_extensions,
                ),
                recursive_unpack: merged(
                    &cleared,
                    "recursive_unpack",
                    params.recursive_unpack,
                    current.recursive_unpack,
                ),
                sfv_verify: merged(
                    &cleared,
                    "sfv_verify",
                    params.sfv_verify,
                    current.sfv_verify,
                ),
                safe_postproc: merged(
                    &cleared,
                    "safe_postproc",
                    params.safe_postproc,
                    current.safe_postproc,
                ),
                delete_par2: merged(
                    &cleared,
                    "delete_par2",
                    params.delete_par2,
                    current.delete_par2,
                ),
                upload_enabled: merged(
                    &cleared,
                    "upload_enabled",
                    params.upload_enabled,
                    current.upload_enabled,
                ),
                upload_remote: merged(
                    &cleared,
                    "upload_remote",
                    params.upload_remote,
                    current.upload_remote,
                ),
            };
            Ok(crate::config_handlers::update_category(
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
        description = "Delete one category. Destructive and refused while unfinished packages still use it; downloaded files are untouched."
    )]
    pub async fn delete_category(&self, Parameters(params): Parameters<IdParams>) -> McpToolResult {
        let result = async {
            let id: rd_core::CategoryId = parse_id(&params.id)?;
            Ok(crate::config_handlers::delete_category(
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
        description = "List the routing rules that decide which category an incoming link lands in, in evaluation order."
    )]
    pub async fn list_category_rules(&self) -> McpToolResult {
        respond(
            crate::config_handlers::list_category_rules(State(self.state.clone()))
                .await
                .map(|rules| rules.0),
        )
    }

    #[tool(
        description = "Create a routing rule. Filters are combined; the lowest priority that matches wins."
    )]
    pub async fn create_category_rule(
        &self,
        Parameters(params): Parameters<CreateCategoryRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = CreateCategoryRuleRequest {
                name: params.name,
                priority: params.priority,
                source: params.source.map(Into::into),
                domain: params.domain,
                protocol: params.protocol,
                extension: params.extension,
                mime_type: params.mime_type,
                name_regex: params.name_regex,
                category_id: parse_id(&params.category_id)?,
                enabled: params.enabled.unwrap_or(true),
            };
            Ok(crate::config_handlers::create_category_rule(
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
        description = "Change one routing rule. Only the fields you pass are changed; name filters to drop in `clear`."
    )]
    pub async fn update_category_rule(
        &self,
        Parameters(params): Parameters<UpdateCategoryRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::CategoryRuleId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_category_rules()
                .await?
                .into_iter()
                .find(|rule| rule.id == id)
                .ok_or_else(|| {
                    ApiError::not_found("category_rule.not_found", "Category rule not found")
                })?;
            let cleared = clearing(
                params.clear.as_ref(),
                &[
                    "source",
                    "domain",
                    "protocol",
                    "extension",
                    "mime_type",
                    "name_regex",
                ],
            )?;
            let request = CreateCategoryRuleRequest {
                name: params.name.unwrap_or(current.name),
                priority: params.priority.unwrap_or(current.priority),
                source: merged(
                    &cleared,
                    "source",
                    params.source.map(Into::into),
                    current.source,
                ),
                domain: merged(&cleared, "domain", params.domain, current.domain),
                protocol: merged(&cleared, "protocol", params.protocol, current.protocol),
                extension: merged(&cleared, "extension", params.extension, current.extension),
                mime_type: merged(&cleared, "mime_type", params.mime_type, current.mime_type),
                name_regex: merged(
                    &cleared,
                    "name_regex",
                    params.name_regex,
                    current.name_regex,
                ),
                category_id: match params.category_id {
                    Some(value) => parse_id(&value)?,
                    None => current.category_id,
                },
                enabled: params.enabled.unwrap_or(current.enabled),
            };
            Ok(crate::config_handlers::update_category_rule(
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

    #[tool(description = "Delete one routing rule. Destructive; existing downloads are untouched.")]
    pub async fn delete_category_rule(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::CategoryRuleId = parse_id(&params.id)?;
            Ok(crate::config_handlers::delete_category_rule(
                State(self.state.clone()),
                AxumPath(id),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }
}
