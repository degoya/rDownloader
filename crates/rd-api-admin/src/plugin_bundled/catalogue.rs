//! What the repositories add to the catalogue of services (RD-1260-02): where each plugin installs
//! from now, where its installed version came from, the update waiting for it, and the plugins
//! only a repository offers.
//!
//! One view instead of the two the plugin manager had — the bundle's services and the
//! repositories' updates — in the bundle's design, with the origin as an attribute of each plugin.

use std::collections::{HashMap, HashSet};

use rd_core::PluginId;
use rd_plugin_host::{
    BundledPackage, BundledService, PluginType, ServiceCategory,
    repository::{Offer, PluginRepositoryService, Update, newest_compatible, replacement},
};

use super::{BundledPluginResponse, BundledServiceResponse, BundledServiceState, PluginOrigin};
use crate::AppState;

/// The repositories' side of the catalogue, read once per request.
pub(super) struct RepositoryView {
    service: PluginRepositoryService,
    offers: Vec<Offer>,
    /// The waiting update per plugin id.
    updates: HashMap<String, Update>,
    /// `(plugin id, version)` of every package a repository delivered here.
    delivered: HashSet<(String, String)>,
}

impl RepositoryView {
    /// Reads offers, updates and deliveries. A part that cannot be read counts as empty: the
    /// catalogue then shows the bundle as it did before the repositories, and says so in the log.
    pub(super) async fn read(state: &AppState) -> Self {
        let service = state.plugin_repositories.clone();
        let offers = service.offers().await.unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the repository offers for the plugin catalogue");
            Vec::new()
        });
        let updates = match service.updates().await {
            Ok(updates) => updates
                .into_iter()
                .map(|update| (update.offer.entry.id.to_string(), update))
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "could not read the plugin updates for the plugin catalogue");
                HashMap::new()
            }
        };
        let delivered = state
            .database
            .list_plugin_repository_installs()
            .await
            .map(|rows| {
                rows.into_iter()
                    .map(|row| (row.plugin_id, row.version))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            service,
            offers,
            updates,
            delivered,
        }
    }

    /// The offer an install of `package` takes now: a [`replacement`] whose host answers.
    pub(super) fn source_of(&self, package: &BundledPackage) -> Option<&Offer> {
        replacement(&self.offers, package).filter(|offer| self.service.offer_reachable(offer))
    }

    /// Where the installed `version` of `id` came from.
    fn installed_origin(&self, id: PluginId, version: &str, bundled: Option<&str>) -> PluginOrigin {
        if self
            .delivered
            .contains(&(id.to_string(), version.to_owned()))
        {
            PluginOrigin::Repository
        } else if bundled == Some(version) {
            PluginOrigin::Bundled
        } else {
            PluginOrigin::Upload
        }
    }

    fn update(&self, id: PluginId) -> Option<crate::plugin_repository_dto::PluginUpdateResponse> {
        self.updates.get(&id.to_string()).cloned().map(Into::into)
    }

    /// One bundled plugin with what the repositories say about it.
    pub(super) fn plugin(
        &self,
        package: &BundledPackage,
        installed: Option<&String>,
        language: &str,
    ) -> BundledPluginResponse {
        let manifest = &package.manifest;
        let source = self.source_of(package);
        BundledPluginResponse {
            id: manifest.id,
            name: package.name(language).to_owned(),
            plugin_type: manifest.plugin_type.as_str().to_owned(),
            version: manifest.version.clone(),
            installed_version: installed.cloned(),
            origin: if source.is_some() {
                PluginOrigin::Repository
            } else {
                PluginOrigin::Bundled
            },
            repository_version: source.map(|offer| offer.entry.version.clone()),
            repository_id: source.map(|offer| offer.repository_id.clone()),
            installed_origin: installed.map(|version| {
                self.installed_origin(manifest.id, version, Some(manifest.version.as_str()))
            }),
            update: installed.and_then(|_| self.update(manifest.id)),
        }
    }

    /// The newest offer this build runs of every plugin no bundled service holds, one service
    /// each, ordered like the bundle's.
    pub(super) fn repository_services(
        &self,
        catalogue: &[BundledService],
        installed: &HashMap<PluginId, String>,
    ) -> Vec<BundledServiceResponse> {
        let mut services: Vec<(ServiceCategory, BundledServiceResponse)> =
            newest_compatible(&self.offers)
                .into_values()
                .filter(|offer| {
                    !catalogue
                        .iter()
                        .any(|service| service.contains(&offer.entry.id))
                })
                .map(|offer| {
                    let category = category(&offer.entry.plugin_type);
                    (
                        category,
                        self.repository_service(offer, category, installed),
                    )
                })
                .collect();
        services.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.name.cmp(&right.1.name))
        });
        services.into_iter().map(|(_, service)| service).collect()
    }

    fn repository_service(
        &self,
        offer: &Offer,
        category: ServiceCategory,
        installed: &HashMap<PluginId, String>,
    ) -> BundledServiceResponse {
        let entry = &offer.entry;
        let installed_version = installed.get(&entry.id).cloned();
        let update = installed_version
            .as_ref()
            .and_then(|_| self.update(entry.id));
        BundledServiceResponse {
            key: entry.id.to_string(),
            name: entry.name.clone(),
            description: String::new(),
            category: category.as_str().to_owned(),
            needs_account: matches!(
                entry.plugin_type,
                PluginType::Auth
                    | PluginType::OAuth
                    | PluginType::RemoteJob
                    | PluginType::Notifier
                    | PluginType::Storage
            ),
            provider: None,
            state: if installed_version.is_some() {
                BundledServiceState::Installed
            } else {
                BundledServiceState::Available
            },
            update_available: update.is_some(),
            plugins: vec![BundledPluginResponse {
                id: entry.id,
                name: entry.name.clone(),
                plugin_type: entry.plugin_type.as_str().to_owned(),
                version: entry.version.clone(),
                installed_origin: installed_version
                    .as_deref()
                    .map(|version| self.installed_origin(entry.id, version, None)),
                installed_version,
                origin: PluginOrigin::Repository,
                repository_version: Some(entry.version.clone()),
                repository_id: Some(offer.repository_id.clone()),
                update,
            }],
        }
    }
}

/// Where a plugin of `kind` is filed when no manifest says more: the index names its type only.
fn category(kind: &PluginType) -> ServiceCategory {
    match kind {
        PluginType::Resolver | PluginType::StreamTransform => ServiceCategory::Hoster,
        PluginType::Notifier => ServiceCategory::Notifications,
        PluginType::Postprocess => ServiceCategory::Postprocess,
        PluginType::Storage => ServiceCategory::Cloud,
        PluginType::Intake | PluginType::Crawler => ServiceCategory::Links,
        PluginType::Enricher => ServiceCategory::Metadata,
        PluginType::RemoteJob => ServiceCategory::RemoteJobs,
        _ => ServiceCategory::Other,
    }
}
