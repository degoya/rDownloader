//! Plugin manager endpoints: listing, trust-on-first-use installation and key management.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_plugin_host::VerifyError;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

use crate::{
    ApiError, AppState,
    dto::{
        IncompatiblePluginResponse, InstalledPluginResponse, MessageResponse,
        PluginDigestRevocationResponse, PluginExecutionResponse, PluginTrustedKeyResponse,
    },
    plugin_lifecycle::PluginInventoryResponse,
};

pub(crate) const MAX_PLUGIN_PACKAGE_BYTES: usize = 65 * 1024 * 1024;

mod install;
mod superseded;
mod trust;

pub use install::*;
pub use superseded::*;
pub use trust::*;

#[utoipa::path(get, path = "/api/v1/plugins", tag = "plugins", responses((status = 200, body = PluginInventoryResponse)))]
pub async fn list_plugins(
    State(state): State<AppState>,
) -> Result<Json<PluginInventoryResponse>, ApiError> {
    // Marking the version that runs is what tells a leftover older version apart from a plugin
    // that is genuinely in use — they looked identical before. Which one runs is the version
    // choice of this start (RD-140-02), newest first where there is none.
    let versions = crate::plugin_lifecycle::loadable_versions(&state).await?;
    let lifecycle = crate::plugin_lifecycle::lifecycles(&state, &versions).await?;
    let running: std::collections::HashMap<String, String> = lifecycle
        .iter()
        .filter_map(|entry| Some((entry.plugin_id.clone(), entry.running_version.clone()?)))
        .collect();
    // One grouped read for every plugin's number of recorded invocations, so the manager can
    // decide whether to offer the diagnostics accordion at all without fetching a single entry.
    // The entries themselves stay on demand; this is the count, not the history.
    let counts: std::collections::HashMap<rd_core::PluginId, i64> = state
        .database
        .plugin_execution_counts()
        .await?
        .into_iter()
        // The store keys entries by the id as text; a row whose id no longer parses belongs to
        // no installed plugin and is dropped rather than failing the inventory.
        .filter_map(|(id, count)| id.parse::<rd_core::PluginId>().ok().map(|id| (id, count)))
        .collect();
    let installed: Vec<InstalledPluginResponse> = state
        .plugins
        .list_installed()
        .await?
        .into_iter()
        .map(|manifest| {
            let mut response = InstalledPluginResponse::from(manifest);
            response.active = running.get(&response.id.to_string()) == Some(&response.version);
            response.execution_count = counts
                .get(&response.id)
                .copied()
                .unwrap_or(0)
                .try_into()
                .unwrap_or(u32::MAX);
            response
        })
        .collect();
    let incompatible = state
        .plugins
        .list_incompatible()
        .await?
        .into_iter()
        .map(IncompatiblePluginResponse::from)
        .collect();
    Ok(Json(PluginInventoryResponse {
        installed,
        incompatible,
        lifecycle,
        automatic_updates_global: crate::plugin_update_policy::automatic_for_all(&state.database)
            .await?,
    }))
}

/// The newest recorded invocations of one plugin.
///
/// Bounded by the store itself, so this cannot ask for an unbounded read.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/{id}/executions",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    responses((status = 200, body = [PluginExecutionResponse]))
)]
pub async fn list_plugin_executions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<PluginExecutionResponse>>, ApiError> {
    let entries = state
        .database
        .plugin_executions(&id, rd_db::MAX_EXECUTIONS_PER_PLUGIN)
        .await?
        .into_iter()
        .map(PluginExecutionResponse::from)
        .collect();
    Ok(Json(entries))
}

/// Removes one installed plugin version.
///
/// The manager offers this for a package this build refuses, so an outdated third-party
/// resolver can be cleared out by hand, and for the older of two installed versions, which
/// installing never removes. Nothing removes such a package on its own: it is the user's
/// artefact, and a plugin that vanishes without a word explains nothing.
///
/// Work that is still bound to the version refuses the removal instead. The whole reason an
/// older version survives an upgrade is that a job already under way keeps the version that
/// started it; taking it away by hand would break exactly what keeping it protects.
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/{id}/{version}",
    tag = "plugins",
    params(
        ("id" = String, Path, description = "Plugin id"),
        ("version" = String, Path, description = "Installed version")
    ),
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, body = MessageResponse),
        (status = 409, body = MessageResponse, description = "Unfinished work is bound to this version")
    )
)]
pub async fn remove_plugin_version(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<MessageResponse>, ApiError> {
    refuse_version_in_use(&state, &id, &version).await?;
    let removed = state
        .plugins
        .remove_version(&id, &version)
        .await
        .map_err(|error| {
            let reason = format!("{error:#}");
            ApiError::bad_request("plugin.remove_failed", reason.clone())
                .with_param("reason", reason)
        })?;
    if !removed {
        return Err(ApiError::not_found(
            "plugin.not_installed",
            "This plugin version is not installed",
        ));
    }
    // The resolver itself goes on the next start, but the provider row must go now: it is what
    // the accounts list offers, and offering a provider whose plugin was just removed would
    // let an account be created that nothing can serve. Installing has always registered its
    // row immediately; removing simply never took it back.
    state.plugins.refresh_providers().await;
    crate::plugin_lifecycle::forget_version(&state, &id, &version).await?;
    announce_plugin(&state, &id, "removed");
    crate::audit::record(
        &state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginRemoved)
            .by(&audit)
            .target("plugin", &id)
            .detail("version", &version),
    )
    .await;
    Ok(Json(MessageResponse::new(
        "plugin.removed",
        "Plugin version removed. Restart to apply.",
    )))
}

/// Refuses with `plugin.version_in_use` while unfinished work is bound to `version`.
///
/// Shared with removing a bundled service, which takes the same versions away.
pub(crate) async fn refuse_version_in_use(
    state: &AppState,
    id: &str,
    version: &str,
) -> Result<(), ApiError> {
    // Asked before the directory goes: a pinned job and a transfer checkpoint both name one
    // exact version, and neither can be rebuilt from the newer one. The blockers are read
    // first, because naming one of them is what lets the reader go and look at it; the count
    // then says how many more there are.
    let blockers = state
        .database
        .plugin_version_blockers(id, version, 2)
        .await?;
    if !blockers.is_empty() {
        let names = blockers.join(", ");
        // The two reads are one predicate apart in time; a job that ended in between must not
        // turn the sentence into "0 unfinished downloads".
        let count = state
            .database
            .plugin_version_usage(id, version)
            .await?
            .max(1);
        let message = if count == 1 {
            format!("This plugin version cannot be removed: {names} is still using it")
        } else {
            format!(
                "This plugin version cannot be removed: {count} unfinished downloads are still using it, including {names}"
            )
        };
        return Err(ApiError::conflict("plugin.version_in_use", message)
            .with_param("count", count)
            .with_param("names", names));
    }
    Ok(())
}

/// Removes one installed version as `remove_plugin_version` does, audited with its `source`.
///
/// Shared by the removals that take several versions at once — a bundled service (RD-180-14)
/// and every superseded version (RD-1140-04) — which refresh the provider rows once at the end
/// rather than per version. The caller has already asked `refuse_version_in_use`. Answers
/// whether the version was still there: gone in between, by another request, there is nothing
/// left to forget or to record.
pub(crate) async fn remove_installed_version(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    manifest: &rd_plugin_host::PluginManifest,
    source: &str,
) -> Result<bool, ApiError> {
    let id = manifest.id.to_string();
    let removed = state
        .plugins
        .remove_version(&id, &manifest.version)
        .await
        .map_err(|error| {
            let reason = format!("{error:#}");
            ApiError::bad_request("plugin.remove_failed", reason.clone())
                .with_param("reason", reason)
        })?;
    if !removed {
        return Ok(false);
    }
    crate::plugin_lifecycle::forget_version(state, &id, &manifest.version).await?;
    announce_plugin(state, &id, "removed");
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginRemoved)
            .by(audit)
            .target("plugin", &id)
            .named(manifest.name.clone())
            .detail("version", &manifest.version)
            .detail("source", source),
    )
    .await;
    Ok(true)
}

/// Tells open clients that the installed set changed.
///
/// `PluginChanged` had no producer at all until this: installing, removing and switching a
/// plugin off are filesystem and settings writes that never pass the database writer, so nothing
/// on the bus ever mentioned them. A plugin installed in one browser tab therefore stayed
/// invisible in every other until somebody reloaded, while the far narrower trust writes — which
/// do go through the writer — announced themselves. It carries the plugin id and what happened,
/// never a manifest: `PluginChanged` is administration-scoped and a manifest names domains and
/// secret slots.
pub(crate) fn announce_plugin(state: &AppState, id: &str, action: &str) {
    // Three events for one write, which is not redundancy. A subscriber receives an event only
    // when it holds that event's exact scope, and this write invalidates lists read at three
    // different ones: the plugin inventory at `Admin`, the provider registry and the
    // notification destinations at `Config`, the post-processing steps and upload destinations
    // at `Queue`. Announcing only on the administration channel left a task-scoped token
    // reading a stale list it was never told had changed — invisible from the interface,
    // because a browser session holds every scope.
    let payload = serde_json::json!({ "resource": "plugin", "plugin_id": id, "action": action });
    for kind in [
        rd_core::EventKind::PluginChanged,
        rd_core::EventKind::PluginCatalogChanged,
        rd_core::EventKind::PostprocessCatalogChanged,
    ] {
        state
            .database
            .broadcast(rd_core::EventEnvelope::new(kind, payload.clone()));
    }
}

/// Body of `PATCH /api/v1/plugins/{id}`.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct PluginEnabledRequest {
    pub enabled: bool,
}

#[utoipa::path(
    patch,
    path = "/api/v1/plugins/{id}",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    request_body = PluginEnabledRequest,
    responses((status = 200, body = MessageResponse))
)]
/// Switches one installed plugin off or back on.
///
/// The plugin stays installed and keeps being listed — otherwise it could not be switched back
/// on — but it is no longer loaded, compiled or executed. Like an update, this takes full effect
/// on the next start, because resolvers and extension hosts are built when their subsystem
/// starts.
pub async fn set_plugin_enabled(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PluginEnabledRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let mut settings = crate::settings_store::stored_settings(&state.database).await?;
    settings.disabled_plugins.retain(|entry| entry != &id);
    if !request.enabled {
        settings.disabled_plugins.push(id.clone());
    }
    // Persisted directly rather than through `apply_settings`: that fans the whole document out
    // to the scheduler, bandwidth, power and the auth service, and on an installation that never
    // saved its settings it would write every default over the running configuration. Nothing in
    // that fan-out reads this list — it is applied when the plugin subsystems start.
    state
        .database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::to_value(&settings).map_err(anyhow::Error::new)?,
        )
        .await?;
    state.plugins.set_disabled(settings.disabled_plugins);
    // A switched-off plugin contributes no provider, the same way startup filters it out. Doing
    // it here too means the accounts list stops offering it at once instead of at the next
    // start — and switching it back on brings it straight back.
    state.plugins.refresh_providers().await;
    announce_plugin(
        &state,
        &id,
        if request.enabled {
            "enabled"
        } else {
            "disabled"
        },
    );
    Ok(Json(if request.enabled {
        MessageResponse::new("plugin.enabled", "Plugin switched on. Restart to apply.")
    } else {
        MessageResponse::new("plugin.disabled", "Plugin switched off. Restart to apply.")
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/plugins/i18n/{locale}",
    tag = "plugins",
    params(("locale" = String, Path,)),
    responses((status = 200, body = serde_json::Value))
)]
pub async fn plugin_messages(
    State(state): State<AppState>,
    Path(locale): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !rd_plugin_host::valid_language(&locale) {
        return Err(ApiError::bad_request(
            "plugin.locale_invalid",
            "Locale must be a two-letter language tag",
        ));
    }
    let bundle = state.plugins.locale_bundle(locale).await?;
    Ok(Json(bundle))
}
