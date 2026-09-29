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
        ToolError::HashMismatch { .. } => ApiError::bad_gateway(code, message),
        // The reason names what went wrong -- a status, a size, or the program an archive
        // failed to deliver under the name the resolver looks for (RD-140-08).
        ToolError::DownloadFailed { name, reason } => ApiError::bad_gateway(code, message)
            .with_param("tool", name)
            .with_param("reason", reason),
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

#[utoipa::path(get, path = "/api/v1/system/media", tag = "system", responses((status = 200, body = crate::dto::MediaStatusResponse)))]
pub async fn media_status(
    State(state): State<AppState>,
) -> Result<Json<crate::dto::MediaStatusResponse>, ApiError> {
    let settings = state.media_settings.read().await.clone();
    // Falls back to defaults rather than refusing, as before: this is a status read that must
    // still answer when a setting is broken, and the accessor reports what it rejected.
    let postprocess: rd_core::PostprocessSettings =
        state.database.service_settings_or_default().await?;
    let vendor = settings.vendor_directory.as_deref();
    let ytdlp = rd_core::locate_tool(settings.media_ytdlp_executable.as_deref(), vendor, "yt-dlp");
    let ffmpeg = rd_media::FfmpegTools::resolve(&settings);
    // An explicit rar_executable configures whichever tool `rar_tool` names; the other one
    // still falls back to the vendor folders and PATH.
    let explicit_rar = postprocess.rar_executable.as_deref();
    let unrar = rd_core::locate_tool(
        explicit_rar.filter(|_| postprocess.rar_tool == "unrar"),
        vendor,
        "unrar",
    );
    let seven_zip = rd_core::locate_tool(
        explicit_rar.filter(|_| postprocess.rar_tool == "7z"),
        vendor,
        "7z",
    );
    let rclone = rd_core::locate_tool(postprocess.rclone_executable.as_deref(), vendor, "rclone");
    let gallery = state.gallery_settings.read().await.clone();
    let gallery_dl =
        rd_core::locate_tool(gallery.gallery_executable.as_deref(), vendor, "gallery-dl");
    let streamlink = rd_stream::locate_streamlink(&*state.stream_settings.read().await);
    // The executable an apprise target may name is per target, so the status shows what a
    // target without one would run.
    let apprise = rd_core::locate_tool(None, vendor, "apprise");
    let (ytdlp, ffmpeg_status, ffprobe, unrar, seven_zip, rclone, gallery_dl, streamlink, apprise) = tokio::join!(
        rd_media::tool_status("yt-dlp", ytdlp.as_ref()),
        rd_media::tool_status("ffmpeg", ffmpeg.ffmpeg.as_ref()),
        rd_media::tool_status("ffprobe", ffmpeg.ffprobe.as_ref()),
        rd_media::tool_status("unrar", unrar.as_ref()),
        rd_media::tool_status("7z", seven_zip.as_ref()),
        rd_media::tool_status("rclone", rclone.as_ref()),
        rd_media::tool_status("gallery-dl", gallery_dl.as_ref()),
        rd_media::tool_status("streamlink", streamlink.as_ref()),
        rd_media::tool_status("apprise", apprise.as_ref())
    );
    // The managed store answers per tool name, so the two extra fields are a lookup rather
    // than another probe: `managed` says whether this application can manage the tool at all,
    // `active_version` which managed version is currently activated. Both are independent of
    // `source`, which says where the binary that would actually run came from.
    let managed_status = state.tools.status().await;
    let convert = move |status: rd_media::ToolStatus| {
        let managed = managed_status.iter().find(|tool| tool.name == status.name);
        crate::dto::MediaToolStatus {
            managed: rd_tools::is_managed_tool(&status.name),
            active_version: managed.and_then(|tool| tool.active_version.clone()),
            compatibility: status.compatibility.into(),
            name: status.name,
            path: status.path,
            version: status.version,
            source: status.source,
        }
    };
    Ok(Json(crate::dto::MediaStatusResponse {
        ytdlp: convert(ytdlp),
        ffmpeg: convert(ffmpeg_status),
        ffprobe: convert(ffprobe),
        unrar: convert(unrar),
        seven_zip: convert(seven_zip),
        rclone: convert(rclone),
        gallery_dl: convert(gallery_dl),
        streamlink: convert(streamlink),
        apprise: convert(apprise),
        vendor_directories: rd_core::vendor_directories(vendor)
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        hosts: settings.media_hosts,
    }))
}
