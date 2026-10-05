//! Which installed version of a plugin runs: activate, stage, roll back, and how updates
//! arrive (RD-140-02).
//!
//! Every choice here is stored and takes effect at the next start, like an update of a running
//! plugin: resolvers and adapters are built once per start, and swapping a component under a
//! running job is a separate job of its own. Only a first install joins the running service
//! (RD-170-12, `plugin_live`). The rules that turn the stored pointers into the version
//! that runs are `rd_plugin_host::default_version`; this module only writes the pointers, and
//! refuses a pointer at a version the next start would refuse to load.

use std::collections::{BTreeMap, HashSet};

use axum::{
    Json,
    extract::{Path, State},
};
use rd_plugin_host::{PluginType, StartedVersions, VersionChoice, default_version};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ApiError, AppState,
    audit::{AuditContext, AuditEvent},
    dto::MessageResponse,
};

mod versions;

pub(crate) use versions::*;

/// How updates for one plugin arrive once an update source offers one.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginUpdatePolicy {
    /// An offered update is shown and installed when the operator clicks it.
    #[default]
    Manual,
    /// An offered update is installed on its own; it still becomes active at a restart.
    Automatic,
}

impl PluginUpdatePolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Automatic => "automatic",
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        if value == "automatic" {
            Self::Automatic
        } else {
            Self::Manual
        }
    }
}

/// Where one plugin's versions stand, as the plugin manager shows it.
#[derive(Serialize, ToSchema)]
pub struct PluginLifecycleResponse {
    pub plugin_id: String,
    /// The version new work runs on from the next start.
    pub active_version: Option<String>,
    /// The version new work runs on right now. Differs from `active_version` until the
    /// service restarts.
    pub running_version: Option<String>,
    /// The version under test: only a download started with it runs it.
    pub staged_version: Option<String>,
    /// What a rollback returns to, while it is still installed.
    pub previous_version: Option<String>,
    /// The plugin's own stored policy. The switch for all plugins
    /// (`automatic_updates_global` on the inventory) overrides a manual one without changing it.
    pub update_policy: PluginUpdatePolicy,
    /// Whether a stored choice waits for a restart to take effect.
    pub restart_required: bool,
}

/// What the plugin manager shows: the packages that run, and the packages that do not.
///
/// A refused package is listed rather than dropped. It is an artefact the user installed;
/// silently omitting it explains nothing about why a third-party hoster stopped working.
#[derive(Serialize, ToSchema)]
pub struct PluginInventoryResponse {
    pub installed: Vec<crate::dto::InstalledPluginResponse>,
    pub incompatible: Vec<crate::dto::IncompatiblePluginResponse>,
    /// One entry per installed plugin id: which version runs, which one is under test, and
    /// how updates arrive (RD-140-02).
    pub lifecycle: Vec<PluginLifecycleResponse>,
    /// Whether the switch for all plugins is on (RD-191-10): every installed plugin then updates
    /// as if set to automatic, whatever its own `update_policy` says.
    pub automatic_updates_global: bool,
}

/// Names one installed version of the plugin.
#[derive(Deserialize, ToSchema)]
pub struct PluginVersionRequest {
    pub version: String,
}

/// Body of `PUT /api/v1/plugins/{id}/lifecycle/policy`.
#[derive(Deserialize, ToSchema)]
pub struct PluginUpdatePolicyRequest {
    pub policy: PluginUpdatePolicy,
}

/// Body of `POST /api/v1/plugins/{id}/lifecycle/trial`.
#[derive(Deserialize, ToSchema)]
pub struct PluginTrialRequest {
    /// The download to run on the staged version the next time it starts.
    pub download_id: rd_core::DownloadId,
}

/// Makes one installed version the one new work runs on from the next start.
///
/// The version it replaces becomes the rollback target. Activating the staged version ends
/// its test: it is simply the active one now.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{id}/lifecycle/activate",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    request_body = PluginVersionRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, body = MessageResponse),
        (status = 409, body = MessageResponse, description = "The version would not load")
    )
)]
pub async fn activate_plugin_version(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
    Json(request): Json<PluginVersionRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (mut choice, versions) = current(&state, &id).await?;
    let manifest = healthy(&state, &id, &request.version).await?;
    let replaced = effective(&versions, Some(&pointer(&choice)));
    if replaced.as_deref() != Some(request.version.as_str()) {
        choice.previous_version = replaced;
    }
    if choice.staged_version.as_deref() == Some(request.version.as_str()) {
        choice.staged_version = None;
    }
    choice.active_version = Some(request.version.clone());
    let event = chosen(&audit, &id, "activated")
        .named(manifest.name)
        .detail("version", &request.version)
        .detail(
            "previous",
            choice.previous_version.as_deref().unwrap_or("-"),
        );
    save(&state, choice).await?;
    crate::audit::record(&state, event).await;
    Ok(Json(
        MessageResponse::new(
            "plugin.version_activated",
            format!(
                "Version {} becomes active at the next start. Restart to apply.",
                request.version
            ),
        )
        .with_param("version", request.version),
    ))
}

/// Puts one installed version under test next to the active one.
///
/// Nothing runs it until a download is started with it (`…/lifecycle/trial`) or it is
/// activated. Only resolver plugins can be tried on a download; for every other type the
/// staged version simply waits for its activation.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{id}/lifecycle/stage",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    request_body = PluginVersionRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, body = MessageResponse),
        (status = 409, body = MessageResponse, description = "The version is active or would not load")
    )
)]
pub async fn stage_plugin_version(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
    Json(request): Json<PluginVersionRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (mut choice, versions) = current(&state, &id).await?;
    let manifest = healthy(&state, &id, &request.version).await?;
    // The version the next start would run. When that is the very version being staged -- a
    // newer version was just installed, and newest wins -- the one that runs now stays active
    // instead, which is the whole point of trying the new one first.
    let mut active = effective(&versions, Some(&pointer(&choice)));
    if active.as_deref() == Some(request.version.as_str()) {
        active =
            running_version(&state, &id, &versions).filter(|running| running != &request.version);
    }
    let candidate = VersionChoice {
        active: active.clone(),
        staged: Some(request.version.clone()),
    };
    if active.is_none() || effective(&versions, Some(&candidate)).is_none() {
        return Err(ApiError::conflict(
            "plugin.version_already_active",
            "The version that runs cannot also be the one under test",
        )
        .with_param("version", request.version));
    }
    // Spelled out even where it only repeats the newest version: from here on the plugin
    // changes version when somebody says so, not when a newer one lands on disk.
    let event = chosen(&audit, &id, "staged")
        .named(manifest.name)
        .detail("version", &request.version)
        .detail("active", active.as_deref().unwrap_or("-"));
    choice.active_version = active;
    choice.staged_version = Some(request.version.clone());
    save(&state, choice).await?;
    crate::audit::record(&state, event).await;
    Ok(Json(
        MessageResponse::new(
            "plugin.version_staged",
            format!(
                "Version {} is under test from the next start. Restart to apply.",
                request.version
            ),
        )
        .with_param("version", request.version),
    ))
}

/// Ends a test without activating the staged version.
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/{id}/lifecycle/stage",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    responses((status = 200, body = MessageResponse), (status = 404, body = MessageResponse))
)]
pub async fn discard_staged_plugin_version(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (mut choice, _) = current(&state, &id).await?;
    let Some(version) = choice.staged_version.take() else {
        return Err(ApiError::not_found(
            "plugin.nothing_staged",
            "No version of this plugin is under test",
        ));
    };
    save(&state, choice).await?;
    let event = chosen(&audit, &id, "staging_discarded").detail("version", &version);
    crate::audit::record(&state, event).await;
    Ok(Json(
        MessageResponse::new(
            "plugin.staging_discarded",
            format!("Version {version} is no longer under test. Restart to apply."),
        )
        .with_param("version", version),
    ))
}

/// Returns to the version that was active before the last activation or rollback.
///
/// One write moves both pointers, so every download that starts after the next start runs on
/// the version rolled back to, and none on a mix. Downloads already pinned to the version
/// rolled away from keep it until they finish; that version stays installed for them.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{id}/lifecycle/rollback",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, body = MessageResponse),
        (status = 409, body = MessageResponse, description = "No earlier version to return to")
    )
)]
pub async fn roll_back_plugin_version(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (mut choice, versions) = current(&state, &id).await?;
    let Some(target) = choice
        .previous_version
        .clone()
        .filter(|version| versions.contains(version))
    else {
        return Err(ApiError::conflict(
            "plugin.no_previous_version",
            "There is no earlier version of this plugin to return to",
        ));
    };
    let manifest = healthy(&state, &id, &target).await?;
    let replaced = effective(&versions, Some(&pointer(&choice)));
    choice.previous_version = replaced.clone().filter(|version| version != &target);
    if choice.staged_version.as_deref() == Some(target.as_str()) {
        choice.staged_version = None;
    }
    choice.active_version = Some(target.clone());
    let event = chosen(&audit, &id, "rolled_back")
        .named(manifest.name)
        .detail("version", &target)
        .detail("replaced", replaced.as_deref().unwrap_or("-"));
    save(&state, choice).await?;
    crate::audit::record(&state, event).await;
    Ok(Json(
        MessageResponse::new(
            "plugin.version_rolled_back",
            format!("Version {target} becomes active again at the next start. Restart to apply."),
        )
        .with_param("version", target),
    ))
}

/// Chooses how updates of this plugin arrive: shown and installed by click, or on their own.
///
/// Stored per plugin; an update source reads it when it has something to offer. An update
/// installed either way becomes active at a restart like any other install.
#[utoipa::path(
    put,
    path = "/api/v1/plugins/{id}/lifecycle/policy",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    request_body = PluginUpdatePolicyRequest,
    responses((status = 200, body = MessageResponse), (status = 404, body = MessageResponse))
)]
pub async fn set_plugin_update_policy(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
    Json(request): Json<PluginUpdatePolicyRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (mut choice, _) = current(&state, &id).await?;
    choice.update_policy = request.policy.as_str().to_owned();
    save(&state, choice).await?;
    let event = chosen(&audit, &id, "update_policy").detail("policy", request.policy.as_str());
    crate::audit::record(&state, event).await;
    Ok(Json(
        MessageResponse::new("plugin.update_policy_saved", "Update policy saved")
            .with_param("policy", request.policy.as_str()),
    ))
}

/// Starts one download with the staged version the next time it runs ("test with new
/// version").
///
/// The download's resolver pin is pointed at the staged version, which is the same mechanism
/// that keeps a running job on the version it started with, so the job stays on the staged
/// version until it finishes. Refused while the download is running, and until the staged
/// version is loaded — staging takes effect at a restart like everything else here.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{id}/lifecycle/trial",
    tag = "plugins",
    params(("id" = String, Path, description = "Plugin id")),
    request_body = PluginTrialRequest,
    responses(
        (status = 200, body = MessageResponse),
        (status = 404, body = MessageResponse),
        (status = 409, body = MessageResponse, description = "Nothing staged, not loaded yet, or the download is running")
    )
)]
pub async fn trial_staged_plugin_version(
    State(state): State<AppState>,
    audit: AuditContext,
    Path(id): Path<String>,
    Json(request): Json<PluginTrialRequest>,
) -> Result<Json<MessageResponse>, ApiError> {
    let (choice, _) = current(&state, &id).await?;
    let Some(version) = choice.staged_version else {
        return Err(ApiError::conflict(
            "plugin.nothing_staged",
            "No version of this plugin is under test",
        ));
    };
    let loaded = state
        .plugins
        .version_choices()
        .get(&id)
        .and_then(|running| running.staged.clone());
    if loaded.as_deref() != Some(version.as_str()) {
        return Err(ApiError::conflict(
            "plugin.staging_restart_required",
            "The version under test is loaded at the next start; restart before trying it",
        )
        .with_param("version", version));
    }
    let manifest = healthy(&state, &id, &version).await?;
    if manifest.plugin_type != PluginType::Resolver {
        return Err(ApiError::conflict(
            "plugin.staging_resolver_only",
            "Only a resolver plugin can be tried on a single download",
        ));
    }
    let pinned = state
        .database
        .pin_download_resolver(
            request.download_id,
            rd_core::ResolverPin {
                plugin_id: manifest.id,
                version: version.clone(),
            },
        )
        .await;
    if let Err(error) = pinned {
        return Err(match rd_db::store_kind(&error) {
            Some(rd_db::StoreErrorKind::NotFound) => crate::error_codes::download_not_found(),
            Some(rd_db::StoreErrorKind::WrongState) => ApiError::conflict(
                "plugin.trial_download_running",
                "A running download keeps the version it started with; pause it first",
            ),
            _ => ApiError::from(error),
        });
    }
    let event = chosen(&audit, &id, "trial")
        .named(manifest.name)
        .detail("version", &version)
        .detail("download_id", request.download_id);
    crate::audit::record(&state, event).await;
    Ok(Json(
        MessageResponse::new(
            "plugin.trial_pinned",
            format!("The download runs on version {version} the next time it starts"),
        )
        .with_param("version", version),
    ))
}
