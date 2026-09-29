//! The bundled packages as the services a person picks from (RD-160-05).
//!
//! A person thinks in services — MEGA, Real-Debrid, Discord — not in the plugins that make one:
//! MEGA is a stream transform, a sign-in, a folder crawler and a login probe. The setup wizard
//! and the plugin manager offer the bundle by service, and installing a service installs every
//! plugin that belongs to it.
//!
//! The grouping is read from the manifests rather than kept in a list: a plugin with a
//! `[provider]` section names a service, and an extension whose `claims` name that provider's
//! slug belongs to it. Everything else is a service of its own.

use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use serde::Serialize;

use crate::{
    PluginInstaller, VerifiedPackage,
    locales::{REQUIRED_LOCALE, parse_locale},
    manifest::{CredentialKindManifest, PluginManifest, PluginType, ProviderKindManifest},
};

/// A package's own name and description in one language.
#[derive(Clone, Debug, Default)]
pub struct BundledText {
    pub name: Option<String>,
    pub description: Option<String>,
}

/// One verified package in the bundle directory.
#[derive(Clone, Debug)]
pub struct BundledPackage {
    pub path: PathBuf,
    pub manifest: PluginManifest,
    /// The texts the package ships, by language. A service that is not installed has no entry
    /// in the plugin catalogue the interface reads, so its name comes from here.
    pub texts: BTreeMap<String, BundledText>,
}

impl BundledPackage {
    pub(crate) fn from_verified(path: PathBuf, package: &VerifiedPackage) -> Self {
        let slug = package.manifest.message_slug();
        let texts = package
            .locales
            .iter()
            .filter_map(|(language, bytes)| {
                let locale = parse_locale(slug, language, bytes).ok()?;
                Some((
                    language.clone(),
                    BundledText {
                        name: locale.name,
                        description: locale.description,
                    },
                ))
            })
            .collect();
        Self {
            path,
            manifest: package.manifest.clone(),
            texts,
        }
    }

    /// The name in `language`, then in English, then the manifest's.
    #[must_use]
    pub fn name(&self, language: &str) -> &str {
        self.text(language, |text| text.name.as_deref())
            .unwrap_or(&self.manifest.name)
    }

    /// The description in `language`, then in English, then the manifest's.
    #[must_use]
    pub fn description(&self, language: &str) -> &str {
        self.text(language, |text| text.description.as_deref())
            .unwrap_or(&self.manifest.metadata.description)
    }

    fn text<'a>(
        &'a self,
        language: &str,
        field: impl Fn(&'a BundledText) -> Option<&'a str>,
    ) -> Option<&'a str> {
        self.texts
            .get(language)
            .and_then(&field)
            .or_else(|| self.texts.get(REQUIRED_LOCALE).and_then(&field))
    }

    /// Whether this plugin cannot do anything before somebody signs in or names a destination:
    /// a provider that takes credentials, a secret slot, a sign-in, a remote job, a notifier
    /// or an upload destination.
    fn needs_account(&self) -> bool {
        let manifest = &self.manifest;
        manifest
            .provider
            .as_ref()
            .is_some_and(|provider| provider.credentials != CredentialKindManifest::NoneRequired)
            || !manifest.capabilities.secrets.is_empty()
            || matches!(
                manifest.plugin_type,
                PluginType::Auth
                    | PluginType::OAuth
                    | PluginType::RemoteJob
                    | PluginType::Notifier
                    | PluginType::Storage
            )
    }
}

/// Where the wizard files a service. The order of the variants is the order it shows them.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceCategory {
    /// A file hoster.
    Hoster,
    /// A multihoster or debrid service: one account, many hosters.
    Multihoster,
    /// A service that runs torrents or downloads at the provider (remote jobs).
    RemoteJobs,
    /// A cloud drive, or an upload destination.
    Cloud,
    /// Link formats and folder crawlers that need no account.
    Links,
    /// Metadata added to links and downloads.
    Metadata,
    /// Where notifications are delivered.
    Notifications,
    /// Steps that run after a download.
    Postprocess,
    /// Anything the categories above do not name.
    Other,
}

impl ServiceCategory {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hoster => "hoster",
            Self::Multihoster => "multihoster",
            Self::RemoteJobs => "remote_jobs",
            Self::Cloud => "cloud",
            Self::Links => "links",
            Self::Metadata => "metadata",
            Self::Notifications => "notifications",
            Self::Postprocess => "postprocess",
            Self::Other => "other",
        }
    }
}

/// One service of the bundle and every package that belongs to it.
#[derive(Clone, Debug)]
pub struct BundledService {
    /// Stable key: the provider slug, or the plugin's own slug for a service without one.
    pub key: String,
    pub category: ServiceCategory,
    /// Whether the service does nothing until an account or a destination is set up. The ones
    /// that need none are the default a fresh installation starts with.
    pub needs_account: bool,
    /// The package that names the service first, then the rest by type and name.
    pub packages: Vec<BundledPackage>,
}

impl BundledService {
    /// The package that names the service: its provider, or its only plugin.
    #[must_use]
    pub fn primary(&self) -> &BundledPackage {
        &self.packages[0]
    }

    /// Whether `id` is one of this service's plugins.
    #[must_use]
    pub fn contains(&self, id: &rd_core::PluginId) -> bool {
        self.packages
            .iter()
            .any(|package| &package.manifest.id == id)
    }
}

/// Groups packages into services, ordered by category and key.
///
/// Two packages of the same id — two versions in the bundle directory — count once, the newest.
#[must_use]
pub fn group_services(packages: &[BundledPackage]) -> Vec<BundledService> {
    let mut newest: BTreeMap<rd_core::PluginId, &BundledPackage> = BTreeMap::new();
    for package in packages {
        let replace = newest.get(&package.manifest.id).is_none_or(|current| {
            let parse = |value: &str| semver::Version::parse(value).ok();
            parse(&package.manifest.version) > parse(&current.manifest.version)
        });
        if replace {
            newest.insert(package.manifest.id, package);
        }
    }
    let providers: HashSet<&str> = newest
        .values()
        .filter_map(|package| package.manifest.provider.as_ref())
        .map(|provider| provider.slug.as_str())
        .collect();
    let mut members: BTreeMap<String, Vec<&BundledPackage>> = BTreeMap::new();
    for package in newest.values().copied() {
        members
            .entry(service_key(&package.manifest, &providers))
            .or_default()
            .push(package);
    }
    let mut services: Vec<BundledService> = members
        .into_iter()
        .map(|(key, mut packages)| {
            packages.sort_by(|left, right| {
                right
                    .manifest
                    .provider
                    .is_some()
                    .cmp(&left.manifest.provider.is_some())
                    .then_with(|| {
                        left.manifest
                            .plugin_type
                            .as_str()
                            .cmp(right.manifest.plugin_type.as_str())
                    })
                    .then_with(|| left.manifest.name.cmp(&right.manifest.name))
            });
            BundledService {
                key,
                category: category(&packages),
                needs_account: packages.iter().any(|package| package.needs_account()),
                packages: packages.into_iter().cloned().collect(),
            }
        })
        .collect();
    services.sort_by(|left, right| {
        left.category
            .cmp(&right.category)
            .then_with(|| left.key.cmp(&right.key))
    });
    services
}

/// The provider slug a plugin serves, or its own slug when it serves none of the bundle's.
fn service_key(manifest: &PluginManifest, providers: &HashSet<&str>) -> String {
    if let Some(provider) = &manifest.provider {
        return provider.slug.clone();
    }
    if let Some(claim) = manifest.extension.as_ref().and_then(|extension| {
        extension
            .claims
            .iter()
            .find(|claim| providers.contains(claim.as_str()))
    }) {
        return claim.clone();
    }
    match manifest.message_slug() {
        "" => manifest.id.to_string(),
        slug => slug.to_owned(),
    }
}

fn category(packages: &[&BundledPackage]) -> ServiceCategory {
    let provider = packages
        .iter()
        .find_map(|package| package.manifest.provider.as_ref());
    let has = |kind: PluginType| {
        packages
            .iter()
            .any(|package| package.manifest.plugin_type == kind)
    };
    if let Some(provider) = provider {
        return if provider.kind == ProviderKindManifest::Multihoster {
            ServiceCategory::Multihoster
        } else if has(PluginType::RemoteJob) {
            ServiceCategory::RemoteJobs
        } else if provider.credentials == CredentialKindManifest::OAuth {
            ServiceCategory::Cloud
        } else {
            ServiceCategory::Hoster
        };
    }
    match packages
        .first()
        .map(|package| &package.manifest.plugin_type)
    {
        Some(PluginType::Notifier) => ServiceCategory::Notifications,
        Some(PluginType::Postprocess) => ServiceCategory::Postprocess,
        Some(PluginType::Storage) => ServiceCategory::Cloud,
        Some(PluginType::Intake | PluginType::Crawler) => ServiceCategory::Links,
        Some(PluginType::Enricher) => ServiceCategory::Metadata,
        Some(PluginType::RemoteJob) => ServiceCategory::RemoteJobs,
        _ => ServiceCategory::Other,
    }
}

impl PluginInstaller {
    /// Replaces the bundle this installer offers; called by the start-up sync.
    pub fn set_bundled(&self, packages: Vec<BundledPackage>) {
        if let Ok(mut bundled) = self.bundled.write() {
            *bundled = packages;
        }
    }

    /// The bundle as services, empty when no bundle directory was found.
    #[must_use]
    pub fn bundled_services(&self) -> Vec<BundledService> {
        self.bundled
            .read()
            .map(|packages| group_services(&packages))
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "bundled_services_tests.rs"]
mod tests;
