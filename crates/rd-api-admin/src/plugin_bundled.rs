//! The bundled plugins by service: what is installed, what is available, and installing a
//! service from the bundle (RD-160-05).
//!
//! A start installs only what the person chose: newer versions of the installed plugins, and on
//! a fresh installation's first start the services that need no account. Everything else in the
//! bundle stays available here — for the setup wizard's "Your services" step and the plugin
//! manager — and installing it takes one request, with no upload and no key to confirm: the
//! packages are the release's own, verified again from the bundle directory as they install.
//! Removing a service takes one request too (RD-180-14), and no later start brings it back:
//! only a fresh installation's first start installs anything new.

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Query, State},
};
use rd_core::PluginId;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{ApiError, AppState, dto::MessageResponse};

/// At most this many services in one install or remove request; the bundle has far fewer.
const MAX_SERVICES_PER_REQUEST: usize = 256;

/// Query of `GET /api/v1/plugins/bundled`.
#[derive(Deserialize, IntoParams)]
pub struct BundledCatalogueQuery {
    /// Language of the names and descriptions (`de`, `en`, …); English when absent or not
    /// shipped by a package.
    #[serde(default)]
    pub locale: Option<String>,
}

/// One plugin of a bundled service.
#[derive(Serialize, ToSchema)]
pub struct BundledPluginResponse {
    pub id: PluginId,
    pub name: String,
    pub plugin_type: String,
    /// The version in the bundle.
    pub version: String,
    /// The newest installed version, `null` when the plugin is not installed.
    pub installed_version: Option<String>,
}

/// How much of a service is installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundledServiceState {
    /// Every plugin of the service is installed.
    Installed,
    /// Some are: installing the service adds the rest.
    Partial,
    /// None is.
    Available,
}

/// One service of the bundle.
#[derive(Serialize, ToSchema)]
pub struct BundledServiceResponse {
    /// Stable key: the provider slug, or the plugin's own slug.
    pub key: String,
    pub name: String,
    pub description: String,
    /// `hoster`, `multihoster`, `remote_jobs`, `cloud`, `links`, `metadata`, `notifications`,
    /// `postprocess` or `other`.
    pub category: String,
    /// Whether the service does nothing until an account or a destination is set up. The ones
    /// that need none are what a fresh installation starts with.
    pub needs_account: bool,
    /// The provider an account for this service is created under, when it has one.
    pub provider: Option<String>,
    pub state: BundledServiceState,
    pub plugins: Vec<BundledPluginResponse>,
}

#[derive(Serialize, ToSchema)]
pub struct BundledCatalogueResponse {
    /// Ordered by category, then key. Empty when the service found no bundle directory.
    pub services: Vec<BundledServiceResponse>,
}

/// Body of `POST /api/v1/plugins/bundled/install`.
#[derive(Deserialize, ToSchema)]
pub struct BundledInstallRequest {
    /// Keys of the services to install, as the catalogue lists them.
    pub services: Vec<String>,
}

/// One plugin that could not be installed, or removed.
#[derive(Serialize, ToSchema)]
pub struct BundledInstallFailure {
    pub service: String,
    pub plugin_id: PluginId,
    pub name: String,
    pub code: String,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
pub struct BundledInstallResponse {
    /// `plugin.bundled_installed` when everything installed runs now,
    /// `plugin.bundled_installed_restart_required` when some of it runs from the next start, or
    /// `plugin.bundled_partly_installed` when something failed.
    pub code: String,
    pub message: String,
    pub installed: Vec<BundledPluginResponse>,
    pub failed: Vec<BundledInstallFailure>,
    /// Whether a plugin installed here runs only from the next start (RD-170-12): an update of
    /// a running plugin, a type that is not loaded while the service runs, or one that did not
    /// load. The setup wizard's accounts step says so.
    pub restart_required: bool,
}

/// Body of `POST /api/v1/plugins/bundled/remove`.
#[derive(Deserialize, ToSchema)]
pub struct BundledRemoveRequest {
    /// Keys of the services to remove, as the catalogue lists them.
    pub services: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct BundledRemoveResponse {
    /// `plugin.bundled_removed` when every named service is gone, or
    /// `plugin.bundled_partly_removed` when something stayed.
    pub code: String,
    pub message: String,
    /// Keys of the services whose plugins were all removed.
    pub removed: Vec<String>,
    /// What stayed: a version an unfinished download is bound to (`plugin.version_in_use`)
    /// keeps its whole service, or a removal that failed.
    pub failed: Vec<BundledInstallFailure>,
}

/// The newest installed version of every installed plugin.
async fn installed_versions(state: &AppState) -> Result<HashMap<PluginId, String>, ApiError> {
    let mut versions: HashMap<PluginId, Vec<String>> = HashMap::new();
    for manifest in state.plugins.list_installed().await? {
        versions
            .entry(manifest.id)
            .or_default()
            .push(manifest.version);
    }
    Ok(versions
        .into_iter()
        .filter_map(|(id, versions)| {
            let loaded: Vec<&str> = versions.iter().map(String::as_str).collect();
            rd_plugin_host::default_version(&loaded, None).map(|newest| (id, newest.to_owned()))
        })
        .collect())
}

/// `de-DE` and `de` both read the German texts.
fn language(locale: Option<&str>) -> String {
    locale
        .and_then(|value| value.split(['-', '_']).next())
        .map(str::to_ascii_lowercase)
        .filter(|value| rd_plugin_host::valid_language(value))
        .unwrap_or_else(|| "en".to_owned())
}

fn plugin_response(
    package: &rd_plugin_host::BundledPackage,
    installed: &HashMap<PluginId, String>,
    language: &str,
) -> BundledPluginResponse {
    BundledPluginResponse {
        id: package.manifest.id,
        name: package.name(language).to_owned(),
        plugin_type: package.manifest.plugin_type.as_str().to_owned(),
        version: package.manifest.version.clone(),
        installed_version: installed.get(&package.manifest.id).cloned(),
    }
}

fn service_response(
    service: &rd_plugin_host::BundledService,
    installed: &HashMap<PluginId, String>,
    language: &str,
) -> BundledServiceResponse {
    let plugins: Vec<BundledPluginResponse> = service
        .packages
        .iter()
        .map(|package| plugin_response(package, installed, language))
        .collect();
    let present = plugins
        .iter()
        .filter(|plugin| plugin.installed_version.is_some())
        .count();
    let primary = service.primary();
    BundledServiceResponse {
        key: service.key.clone(),
        name: primary.name(language).to_owned(),
        description: primary.description(language).to_owned(),
        category: service.category.as_str().to_owned(),
        needs_account: service.needs_account,
        provider: primary
            .manifest
            .provider
            .as_ref()
            .map(|provider| provider.slug.clone()),
        state: if present == plugins.len() {
            BundledServiceState::Installed
        } else if present == 0 {
            BundledServiceState::Available
        } else {
            BundledServiceState::Partial
        },
        plugins,
    }
}

/// The bundle by service, with what of it is installed.
#[utoipa::path(
    get,
    path = "/api/v1/plugins/bundled",
    tag = "plugins",
    params(BundledCatalogueQuery),
    responses((status = 200, body = BundledCatalogueResponse))
)]
pub async fn list_bundled_services(
    State(state): State<AppState>,
    Query(query): Query<BundledCatalogueQuery>,
) -> Result<Json<BundledCatalogueResponse>, ApiError> {
    let language = language(query.locale.as_deref());
    let installed = installed_versions(&state).await?;
    let services = state
        .plugins
        .bundled_services()
        .iter()
        .map(|service| service_response(service, &installed, &language))
        .collect();
    Ok(Json(BundledCatalogueResponse { services }))
}

/// The named services, every one looked up before anything changes, so a request naming an
/// unknown one changes nothing.
fn chosen_services<'a>(
    catalogue: &'a [rd_plugin_host::BundledService],
    keys: &[String],
) -> Result<Vec<&'a rd_plugin_host::BundledService>, ApiError> {
    if keys.is_empty() || keys.len() > MAX_SERVICES_PER_REQUEST {
        return Err(ApiError::bad_request(
            "plugin.bundled_services_invalid",
            format!("Name between 1 and {MAX_SERVICES_PER_REQUEST} services"),
        )
        .with_param("max", MAX_SERVICES_PER_REQUEST));
    }
    keys.iter()
        .map(|key| {
            catalogue
                .iter()
                .find(|service| &service.key == key)
                .ok_or_else(|| {
                    ApiError::not_found(
                        "plugin.bundled_service_unknown",
                        format!("The bundle has no service called {key}"),
                    )
                    .with_param("service", key)
                })
        })
        .collect()
}

/// Installs the plugins of the named services that are not installed yet.
///
/// One plugin that fails does not stop the others: the answer lists what was installed and what
/// was not, and the person sees both. Every service is looked up before anything installs, so a
/// request naming an unknown one changes nothing. Like every install, the plugins' provider rows
/// are live at once — the accounts step can offer them straight away — and a first install of a
/// resolver or a sign-in plugin runs at once too (RD-170-12); what does not says so in
/// `restart_required`.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/bundled/install",
    tag = "plugins",
    request_body = BundledInstallRequest,
    responses(
        (status = 200, body = BundledInstallResponse),
        (status = 400, body = MessageResponse),
        (status = 404, body = MessageResponse, description = "The bundle has no service of that key")
    )
)]
pub async fn install_bundled_services(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<BundledInstallRequest>,
) -> Result<Json<BundledInstallResponse>, ApiError> {
    let catalogue = state.plugins.bundled_services();
    let chosen = chosen_services(&catalogue, &request.services)?;
    let installed = installed_versions(&state).await?;
    // Installed ones are skipped, and so is a plugin two named services share.
    let mut done: HashSet<PluginId> = installed.keys().copied().collect();
    let mut response = BundledInstallResponse {
        code: String::new(),
        message: String::new(),
        installed: Vec::new(),
        failed: Vec::new(),
        restart_required: false,
    };
    for service in chosen {
        for package in &service.packages {
            if !done.insert(package.manifest.id) {
                continue;
            }
            match install_package(&state, &audit, package).await {
                Ok((installed, running)) => {
                    response.restart_required |= !running;
                    response.installed.push(BundledPluginResponse {
                        id: installed.manifest.id,
                        name: installed.manifest.name,
                        plugin_type: installed.manifest.plugin_type.as_str().to_owned(),
                        installed_version: Some(installed.manifest.version.clone()),
                        version: installed.manifest.version,
                    });
                }
                Err(error) => response.failed.push(BundledInstallFailure {
                    service: service.key.clone(),
                    plugin_id: package.manifest.id,
                    name: package.manifest.name.clone(),
                    code: error.code().to_owned(),
                    message: error.message().to_owned(),
                }),
            }
        }
    }
    let result = if !response.failed.is_empty() {
        MessageResponse::new(
            "plugin.bundled_partly_installed",
            "Some plugins could not be installed",
        )
    } else if response.restart_required {
        MessageResponse::new(
            "plugin.bundled_installed_restart_required",
            "Services installed; some of their plugins run from the next start",
        )
    } else {
        MessageResponse::new("plugin.bundled_installed", "Services installed and running")
    };
    response.code = result.code;
    response.message = result.message;
    Ok(Json(response))
}

/// Installs one package; the flag says whether it runs now.
async fn install_package(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    package: &rd_plugin_host::BundledPackage,
) -> Result<(rd_plugin_host::InstalledPackage, bool), ApiError> {
    let installed = state
        .plugins
        .install(package.path.clone())
        .await
        .map_err(crate::plugin_handlers::install_error)?;
    let running = crate::plugin_handlers::register_installed(state, &installed).await?;
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginInstalled)
            .by(audit)
            .target("plugin", installed.manifest.id)
            .named(installed.manifest.name.clone())
            .detail("version", &installed.manifest.version)
            .detail("source", "bundled"),
    )
    .await;
    Ok((installed, running))
}

/// Removes every installed version of every plugin of the named services (RD-180-14).
///
/// What the setup wizard's "Your services" step does with an unticked service. Like a delete in
/// the plugin manager, the provider rows go at once and the plugins stop at the next start. A
/// version an unfinished download is bound to refuses with `plugin.version_in_use` and keeps its
/// whole service — half a service would be a sign-in without its hoster — while the others
/// proceed; one that is not installed is skipped.
#[utoipa::path(
    post,
    path = "/api/v1/plugins/bundled/remove",
    tag = "plugins",
    request_body = BundledRemoveRequest,
    responses(
        (status = 200, body = BundledRemoveResponse),
        (status = 400, body = MessageResponse),
        (status = 404, body = MessageResponse, description = "The bundle has no service of that key")
    )
)]
pub async fn remove_bundled_services(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<BundledRemoveRequest>,
) -> Result<Json<BundledRemoveResponse>, ApiError> {
    let catalogue = state.plugins.bundled_services();
    let chosen = chosen_services(&catalogue, &request.services)?;
    let installed = state.plugins.list_installed().await?;
    let mut response = BundledRemoveResponse {
        code: String::new(),
        message: String::new(),
        removed: Vec::new(),
        failed: Vec::new(),
    };
    let mut changed = false;
    for service in chosen {
        if response.removed.contains(&service.key) {
            continue;
        }
        let versions: Vec<&rd_plugin_host::PluginManifest> = installed
            .iter()
            .filter(|manifest| service.contains(&manifest.id))
            .collect();
        if versions.is_empty() {
            continue;
        }
        let failure =
            |manifest: &rd_plugin_host::PluginManifest, error: ApiError| BundledInstallFailure {
                service: service.key.clone(),
                plugin_id: manifest.id,
                name: manifest.name.clone(),
                code: error.code().to_owned(),
                message: error.message().to_owned(),
            };
        let mut refused = false;
        for manifest in versions.iter().copied() {
            let id = manifest.id.to_string();
            if let Err(error) =
                crate::plugin_handlers::refuse_version_in_use(&state, &id, &manifest.version).await
            {
                response.failed.push(failure(manifest, error));
                refused = true;
            }
        }
        if refused {
            continue;
        }
        let mut complete = true;
        for manifest in versions {
            match remove_version(&state, &audit, manifest).await {
                Ok(()) => changed = true,
                Err(error) => {
                    response.failed.push(failure(manifest, error));
                    complete = false;
                }
            }
        }
        if complete {
            response.removed.push(service.key.clone());
        }
    }
    if changed {
        // Now, as in the plugin manager: the accounts step must not offer a removed provider.
        state.plugins.refresh_providers().await;
    }
    let result = if response.failed.is_empty() {
        MessageResponse::new(
            "plugin.bundled_removed",
            "Services removed; their plugins stop at the next start",
        )
    } else {
        MessageResponse::new(
            "plugin.bundled_partly_removed",
            "Some services could not be removed",
        )
    };
    response.code = result.code;
    response.message = result.message;
    Ok(Json(response))
}

/// Removes one installed version of a bundled plugin, as `remove_plugin_version` does.
async fn remove_version(
    state: &AppState,
    audit: &crate::audit::AuditContext,
    manifest: &rd_plugin_host::PluginManifest,
) -> Result<(), ApiError> {
    let id = manifest.id.to_string();
    let removed = state
        .plugins
        .remove_version(&id, &manifest.version)
        .await
        .map_err(|error| ApiError::bad_request("plugin.remove_failed", format!("{error:#}")))?;
    if !removed {
        // Gone in between, by another request: nothing left to forget or to record.
        return Ok(());
    }
    crate::plugin_lifecycle::forget_version(state, &id, &manifest.version).await?;
    crate::plugin_handlers::announce_plugin(state, &id, "removed");
    crate::audit::record(
        state,
        crate::audit::AuditEvent::success(rd_core::AuditAction::PluginRemoved)
            .by(audit)
            .target("plugin", &id)
            .named(manifest.name.clone())
            .detail("version", &manifest.version)
            .detail("source", "bundled"),
    )
    .await;
    Ok(())
}
