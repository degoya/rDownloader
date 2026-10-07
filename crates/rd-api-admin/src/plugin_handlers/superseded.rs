//! Removing every superseded version at once (RD-1140-04).
//!
//! Installing never removes the older version, so a plugin updated a few times carries one
//! leftover per update, and the manager removed them one confirmation at a time. These routes
//! take all of them — of every plugin, or of one — by the rules of removing one: the version that
//! runs never goes, a version unfinished work is bound to stays, and every version that stays is
//! named with the reason, as a stable code.

use std::collections::HashMap;

use serde::Serialize;

use super::*;
use crate::plugin_lifecycle::PluginLifecycleResponse;

/// One installed version of a plugin.
#[derive(Serialize, ToSchema)]
pub struct PluginVersionEntry {
    pub plugin_id: rd_core::PluginId,
    pub name: String,
    pub version: String,
}

/// A superseded version that stayed, and why.
#[derive(Serialize, ToSchema)]
pub struct KeptPluginVersion {
    pub plugin_id: rd_core::PluginId,
    pub name: String,
    pub version: String,
    /// `plugin.version_in_use` while unfinished work is bound to it,
    /// `plugin.version_next_start` when it is the one the next start loads,
    /// `plugin.version_under_test` while it is under test, or `plugin.remove_failed`.
    pub reason: MessageResponse,
}

#[derive(Serialize, ToSchema)]
pub struct SupersededRemovalResponse {
    /// `plugin.superseded_removed` when every superseded version is gone,
    /// `plugin.superseded_partly_removed` when some stayed, or `plugin.superseded_none` when
    /// there was none.
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub params: rd_core::MessageParams,
    pub removed: Vec<PluginVersionEntry>,
    pub kept: Vec<KeptPluginVersion>,
}

/// Removes every superseded version of every installed plugin.
///
/// A version is superseded when another version of the same plugin is the one that runs. The
/// one that runs never goes; neither does the one the next start loads — right after an update
/// that is the new one, and taking it would undo the update — nor the one under test, nor one
/// unfinished work is bound to. Each of those is listed under `kept` with its reason.
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/superseded",
    tag = "plugins",
    responses((status = 200, body = SupersededRemovalResponse))
)]
pub async fn remove_superseded_plugin_versions(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
) -> Result<Json<SupersededRemovalResponse>, ApiError> {
    remove_superseded(&state, &audit, None).await.map(Json)
}

/// Removes every superseded version of one plugin, by the rules of
/// `remove_superseded_plugin_versions`.
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/{id}/superseded",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    responses(
        (status = 200, body = SupersededRemovalResponse),
        (status = 404, body = MessageResponse)
    )
)]
pub async fn remove_superseded_versions_of_plugin(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path(id): Path<String>,
) -> Result<Json<SupersededRemovalResponse>, ApiError> {
    remove_superseded(&state, &audit, Some(&id)).await.map(Json)
}

/// The removal behind both routes; `only` narrows it to one plugin id.
///
/// Shared with the MCP tool, which takes the plugin id as an optional argument.
pub async fn remove_superseded(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    only: Option<&str>,
) -> Result<SupersededRemovalResponse, ApiError> {
    let versions = crate::plugin_lifecycle::loadable_versions(state).await?;
    let lifecycle: HashMap<String, PluginLifecycleResponse> =
        crate::plugin_lifecycle::lifecycles(state, &versions)
            .await?
            .into_iter()
            .map(|entry| (entry.plugin_id.clone(), entry))
            .collect();
    let running_choices = state.plugins.version_choices();
    let installed: Vec<rd_plugin_host::PluginManifest> = state
        .plugins
        .list_installed()
        .await?
        .into_iter()
        .filter(|manifest| only.is_none_or(|id| manifest.id.to_string() == id))
        .collect();
    if only.is_some() && installed.is_empty() {
        return Err(ApiError::not_found(
            "plugin.not_installed",
            "This plugin is not installed",
        ));
    }
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for manifest in &installed {
        let id = manifest.id.to_string();
        // A plugin none of whose versions runs or is chosen has nothing that superseded the
        // others: they are not leftovers of an update, and this removal leaves them alone.
        let Some(entry) = lifecycle.get(&id) else {
            continue;
        };
        let Some(card) = entry
            .running_version
            .as_ref()
            .or(entry.active_version.as_ref())
        else {
            continue;
        };
        if &manifest.version == card {
            continue;
        }
        let running_staged = running_choices
            .get(&id)
            .and_then(|choice| choice.staged.as_deref());
        let outcome = match chosen(entry, running_staged, &manifest.version) {
            Some(reason) => Err(reason),
            None => remove_one(state, audit, manifest).await,
        };
        let named = PluginVersionEntry {
            plugin_id: manifest.id,
            name: manifest.name.clone(),
            version: manifest.version.clone(),
        };
        match outcome {
            Ok(()) => removed.push(named),
            Err(error) => kept.push(KeptPluginVersion {
                plugin_id: named.plugin_id,
                name: named.name,
                version: named.version,
                reason: error.into_message(),
            }),
        }
    }
    if !removed.is_empty() {
        // Once for the whole batch, as the bundled removal does: the accounts list must not
        // offer a provider whose last version just went.
        state.plugins.refresh_providers().await;
    }
    let result = if removed.is_empty() && kept.is_empty() {
        MessageResponse::new("plugin.superseded_none", "No superseded plugin versions")
    } else if kept.is_empty() {
        MessageResponse::new(
            "plugin.superseded_removed",
            format!("{} superseded plugin versions removed", removed.len()),
        )
        .with_param("count", removed.len())
    } else {
        MessageResponse::new(
            "plugin.superseded_partly_removed",
            format!(
                "{} superseded plugin versions removed, {} kept",
                removed.len(),
                kept.len()
            ),
        )
        .with_param("count", removed.len())
        .with_param("kept", kept.len())
    };
    Ok(SupersededRemovalResponse {
        code: result.code,
        message: result.message,
        params: result.params,
        removed,
        kept,
    })
}

/// Why a version that does not run is still chosen, if it is: the next start loads it, or it is
/// under test, stored or running.
fn chosen(
    entry: &PluginLifecycleResponse,
    running_staged: Option<&str>,
    version: &str,
) -> Option<ApiError> {
    if entry.active_version.as_deref() == Some(version) {
        return Some(
            ApiError::conflict(
                "plugin.version_next_start",
                format!("Version {version} runs from the next start"),
            )
            .with_param("version", version),
        );
    }
    if entry.staged_version.as_deref() == Some(version) || running_staged == Some(version) {
        return Some(
            ApiError::conflict(
                "plugin.version_under_test",
                format!("Version {version} is under test"),
            )
            .with_param("version", version),
        );
    }
    None
}

/// One superseded version: refused while work is bound to it, removed otherwise.
async fn remove_one(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    manifest: &rd_plugin_host::PluginManifest,
) -> Result<(), ApiError> {
    refuse_version_in_use(state, &manifest.id.to_string(), &manifest.version).await?;
    remove_installed_version(state, audit, manifest, "superseded").await?;
    Ok(())
}
