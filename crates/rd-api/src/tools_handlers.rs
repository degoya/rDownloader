//! Managed external tools over REST (RD-102-02).
//!
//! Reading the store is a `Read` operation; installing, activating and rolling back are
//! `Config`, the same class as writing the settings — they change what the service executes,
//! which is a configuration decision and not a queue one.
//!
//! Every failure carries one of the stable `tools.*` codes from [`rd_tools::ToolError`], so
//! the web interface can tell an untrusted manifest from a slow mirror instead of showing one
//! sentence for both.

use axum::{
    Json,
    extract::{Path, State},
};

use crate::{AppState, error::ApiError};

/// Turns a tool failure into the REST error carrying its stable code.
///
/// The status matters as much as the code: an untrusted or stale manifest is a *server-side*
/// trust decision the client cannot fix by asking differently, a missing version is a bad
/// request, and a failed download is an upstream problem.
fn to_api_error(error: rd_tools::ToolError) -> ApiError {
    use rd_tools::ToolError;
    let code = error.code();
    let message = error.to_string();
    match error {
        ToolError::ManifestUntrusted(_) | ToolError::ManifestStale(_) => {
            ApiError::bad_gateway(code, message)
        }
        ToolError::HashMismatch { .. } | ToolError::DownloadFailed { .. } => {
            ApiError::bad_gateway(code, message)
        }
        ToolError::VersionNotInstalled { .. } | ToolError::NothingToRollBackTo { .. } => {
            ApiError::not_found(code, message)
        }
        ToolError::NotManaged(_) => ApiError::bad_request(code, message),
        ToolError::NoRelease { .. } => ApiError::unprocessable(code, message),
        ToolError::Disabled => ApiError::conflict(code, message),
        ToolError::InUse { .. } => ApiError::conflict(code, message),
        ToolError::Other(other) => other.into(),
    }
}

#[utoipa::path(get, path = "/api/v1/system/tools", tag = "system", responses((status = 200, body = crate::dto::ManagedToolsResponse)))]
pub async fn list_managed_tools(
    State(state): State<AppState>,
) -> Result<Json<crate::dto::ManagedToolsResponse>, ApiError> {
    let manifest = state.tools.manifest();
    // Falls back to defaults rather than refusing, as before: this only decides what the tool
    // list shows, and the accessor reports a malformed blob.
    let settings: rd_core::ManagedToolSettings =
        state.database.service_settings_or_default().await?;
    let tools = state
        .tools
        .status()
        .await
        .into_iter()
        .map(|tool| crate::dto::ManagedToolInfo {
            name: tool.name,
            active_version: tool.active_version,
            active_path: tool.active_path,
            installed_versions: tool.installed_versions,
            available_version: tool.available_version,
            can_roll_back: tool.can_roll_back,
        })
        .collect();
    Ok(Json(crate::dto::ManagedToolsResponse {
        enabled: settings.managed_tools_enabled,
        platform: rd_tools::platform::current().to_owned(),
        manifest_sequence: manifest.sequence,
        manifest_issued_at: manifest.issued_at.to_rfc3339(),
        manifest_url: settings.managed_tools_manifest_url,
        tools,
    }))
}

#[utoipa::path(post, path = "/api/v1/system/tools/manifest/refresh", tag = "system", responses((status = 200, body = crate::dto::ManagedToolsResponse)))]
pub async fn refresh_tool_manifest(
    State(state): State<AppState>,
) -> Result<Json<crate::dto::ManagedToolsResponse>, ApiError> {
    state.tools.refresh_manifest().await.map_err(to_api_error)?;
    list_managed_tools(State(state)).await
}

#[utoipa::path(
    post,
    path = "/api/v1/system/tools/{name}/install",
    tag = "system",
    params(("name" = String, Path, description = "Managed tool name")),
    request_body = crate::dto::ManagedToolVersionRequest,
    responses((status = 200, body = crate::dto::ManagedToolsResponse))
)]
pub async fn install_managed_tool(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<crate::dto::ManagedToolVersionRequest>,
) -> Result<Json<crate::dto::ManagedToolsResponse>, ApiError> {
    state
        .tools
        .install(&name, request.version.as_deref())
        .await
        .map_err(to_api_error)?;
    list_managed_tools(State(state)).await
}

#[utoipa::path(
    post,
    path = "/api/v1/system/tools/{name}/activate",
    tag = "system",
    params(("name" = String, Path, description = "Managed tool name")),
    request_body = crate::dto::ManagedToolVersionRequest,
    responses((status = 200, body = crate::dto::ManagedToolsResponse))
)]
pub async fn activate_managed_tool(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(request): Json<crate::dto::ManagedToolVersionRequest>,
) -> Result<Json<crate::dto::ManagedToolsResponse>, ApiError> {
    let version = request.version.ok_or_else(|| {
        ApiError::bad_request(
            "tools.version_not_installed",
            "Name the version to activate",
        )
    })?;
    state
        .tools
        .activate(&name, &version)
        .await
        .map_err(to_api_error)?;
    list_managed_tools(State(state)).await
}

#[utoipa::path(
    post,
    path = "/api/v1/system/tools/{name}/rollback",
    tag = "system",
    params(("name" = String, Path, description = "Managed tool name")),
    responses((status = 200, body = crate::dto::ManagedToolsResponse))
)]
pub async fn rollback_managed_tool(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<crate::dto::ManagedToolsResponse>, ApiError> {
    state.tools.rollback(&name).await.map_err(to_api_error)?;
    list_managed_tools(State(state)).await
}
