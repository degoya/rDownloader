//! Post-processing pipeline endpoints: queue view, script catalogue, category defaults.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::{CategoryId, PackageState, PostprocessLevel};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    dto::{CategoryPostprocessRequest, PostprocessQueueEntry, PostprocessScriptsResponse},
};

/// Level/script change parsed from a request (`Some(None)` = inherit again).
#[derive(Clone, Debug, Default)]
pub struct PostprocessChange {
    pub level: Option<Option<PostprocessLevel>>,
    pub script: Option<Option<String>>,
}

/// Turns the request fields (value + clear flag) into a change description.
pub fn postprocess_change(
    level: Option<PostprocessLevel>,
    clear_level: bool,
    script: Option<String>,
    clear_script: bool,
) -> Result<PostprocessChange, ApiError> {
    Ok(PostprocessChange {
        level: if clear_level {
            Some(None)
        } else {
            level.map(Some)
        },
        script: if clear_script {
            Some(None)
        } else {
            validate_script_name(script)?.map(Some)
        },
    })
}

/// Normalises a cleanup list the same way the global setting is normalised, so a category
/// override and the global list behave identically at extraction time.
pub fn normalize_cleanup_extensions(values: Vec<String>) -> Result<Vec<String>, ApiError> {
    let normalized: Vec<String> = values
        .iter()
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    if let Some(bad) = normalized
        .iter()
        .find(|value| value.len() > 10 || !value.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return Err(ApiError::bad_request(
            "settings.cleanup_extension_invalid",
            format!("Cleanup extension '{bad}' must be 1-10 alphanumeric characters"),
        )
        .with_param("value", bad));
    }
    Ok(normalized)
}

/// Script names are bare file names inside the scripts directory (no separators).
pub fn validate_script_name(value: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let valid = trimmed.len() <= 128
        && !trimmed.starts_with('.')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(ApiError::bad_request(
            "postprocess.script_name_invalid",
            "Script name may only contain letters, digits, '.', '_' and '-' (max 128 characters)",
        )
        .with_param("max", 128));
    }
    Ok(Some(trimmed.to_owned()))
}

#[utoipa::path(get, path = "/api/v1/postprocess/queue", tag = "downloads", responses((status = 200, body = [PostprocessQueueEntry])))]
pub async fn list_postprocess_queue(
    State(state): State<AppState>,
) -> Result<Json<Vec<PostprocessQueueEntry>>, ApiError> {
    let pending = state.extraction.pending().await;
    let entries = state
        .database
        .list_packages()
        .await?
        .into_iter()
        .filter(|package| {
            package.state == PackageState::Postprocessing || pending.contains(&package.id)
        })
        .map(|package| PostprocessQueueEntry {
            pending: package.state != PackageState::Postprocessing,
            package_id: package.id,
            name: package.name,
            state: package.state,
            stage: package.postprocess.stage,
            percent: package.postprocess.percent,
            current: package.postprocess.current,
        })
        .collect();
    Ok(Json(entries))
}

#[utoipa::path(get, path = "/api/v1/postprocess/scripts", tag = "downloads", responses((status = 200, body = PostprocessScriptsResponse)))]
pub async fn list_postprocess_scripts(
    State(state): State<AppState>,
) -> Result<Json<PostprocessScriptsResponse>, ApiError> {
    let directory = state.extraction.scripts_directory().await?;
    let mut scripts = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(&directory).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let is_file = entry.file_type().await.is_ok_and(|kind| kind.is_file());
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_file && validate_script_name(Some(name.clone())).is_ok_and(|v| v.is_some()) {
                scripts.push(name);
            }
        }
    }
    scripts.sort();
    Ok(Json(PostprocessScriptsResponse {
        directory: directory.to_string_lossy().into_owned(),
        scripts,
    }))
}

/// One installed post-processing step plugin, for the settings and category editors.
#[derive(Serialize, ToSchema)]
pub struct PostprocessPluginStep {
    /// The value a category or the global list stores.
    pub plugin_id: String,
    /// Default display name. The interface prefers the plugin's own localised name.
    pub name: String,
    pub version: String,
}

#[utoipa::path(get, path = "/api/v1/postprocess/plugin-steps", tag = "downloads", responses((status = 200, body = [PostprocessPluginStep])))]
pub async fn list_plugin_steps(State(state): State<AppState>) -> Json<Vec<PostprocessPluginStep>> {
    Json(
        state
            .plugin_steps
            .list()
            .into_iter()
            .map(|step| PostprocessPluginStep {
                plugin_id: step.plugin_id,
                name: step.name,
                version: step.version,
            })
            .collect(),
    )
}

/// One installed upload destination plugin, for the post-processing settings.
#[derive(Serialize, ToSchema)]
pub struct UploadDestination {
    /// The prefix an upload target uses: `plugin:<plugin_id>/<destination>`.
    pub plugin_id: String,
    pub name: String,
    pub version: String,
}

#[utoipa::path(get, path = "/api/v1/postprocess/upload-destinations", tag = "downloads", responses((status = 200, body = [UploadDestination])))]
pub async fn list_upload_destinations(
    State(state): State<AppState>,
) -> Json<Vec<UploadDestination>> {
    Json(
        state
            .storage_destinations
            .list()
            .into_iter()
            .map(|destination| UploadDestination {
                plugin_id: destination.plugin_id,
                name: destination.name,
                version: destination.version,
            })
            .collect(),
    )
}

#[utoipa::path(patch, path = "/api/v1/categories/{id}/postprocess", tag = "configuration", params(("id" = rd_core::CategoryId, Path)), request_body = CategoryPostprocessRequest, responses((status = 200, body = rd_core::Category), (status = 404)))]
pub async fn update_category_postprocess(
    State(state): State<AppState>,
    Path(id): Path<CategoryId>,
    Json(request): Json<CategoryPostprocessRequest>,
) -> Result<Json<rd_core::Category>, ApiError> {
    let script = validate_script_name(request.script)?;
    let cleanup_extensions = request
        .cleanup_extensions
        .map(normalize_cleanup_extensions)
        .transpose()?;
    let upload_remote = crate::dto::normalize_upload_remote(request.upload_remote)?;
    let category = state
        .database
        .update_category_postprocess(
            id,
            rd_db::CategoryPostprocess {
                level: request.postprocess_level,
                script,
                cleanup_extensions,
                recursive_unpack: request.recursive_unpack,
                sfv_verify: request.sfv_verify,
                safe_postproc: request.safe_postproc,
                delete_par2: request.delete_par2,
                plugin_steps: request.plugin_steps,
                upload_enabled: request.upload_enabled,
                upload_remote,
            },
        )
        .await
        .map_err(|error| {
            crate::error_codes::store_not_found(&error, "category.not_found", "Category not found")
        })?;
    Ok(Json(category))
}

#[cfg(test)]
mod tests {
    use super::validate_script_name;

    #[test]
    fn rejects_paths_and_hidden_files() {
        assert!(validate_script_name(Some("../evil.sh".to_owned())).is_err());
        assert!(validate_script_name(Some("dir/run.sh".to_owned())).is_err());
        assert!(validate_script_name(Some(".hidden".to_owned())).is_err());
        assert_eq!(
            validate_script_name(Some(" rename-files.py ".to_owned())).expect("valid"),
            Some("rename-files.py".to_owned())
        );
        assert_eq!(
            validate_script_name(Some("  ".to_owned())).expect("empty"),
            None
        );
    }
}
