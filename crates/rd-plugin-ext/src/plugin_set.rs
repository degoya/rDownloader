//! The shape the extension adapters share (RD-1120-12): the installed plugins of one type,
//! built from the registry the adapters share, and — for the sign-in adapters — the index of
//! the provider slugs they claim.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::Result;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry, VerifiedPackage,
    extension::{
        AuthProvider, IntakeParser, MetadataEnricher, NotifierPlugin, OAuthProvider,
        PostprocessPlugin, StoragePlugin,
    },
};

use crate::provider::{ProviderError, ProviderResult};

/// An extension instance the registry builds, one per installed package of its type.
pub trait ExtensionPlugin: Sized {
    /// The plugin type whose packages build it.
    const PLUGIN_TYPE: PluginType;

    /// Builds the instance for one verified package.
    fn build(package: &VerifiedPackage, host: Option<Arc<dyn ResolverHost>>) -> Result<Self>;
}

/// An extension that signs in to a provider and is looked up by the slug it claims.
pub trait ClaimingPlugin: ExtensionPlugin {
    /// What the warning about a plugin claiming nothing calls it.
    const KIND: &'static str;
}

macro_rules! extension_plugin {
    ($plugin:ty, $plugin_type:ident) => {
        impl ExtensionPlugin for $plugin {
            const PLUGIN_TYPE: PluginType = PluginType::$plugin_type;

            fn build(
                package: &VerifiedPackage,
                host: Option<Arc<dyn ResolverHost>>,
            ) -> Result<Self> {
                Self::new(package.manifest.clone(), &package.component, host)
            }
        }
    };
}

extension_plugin!(AuthProvider, Auth);
extension_plugin!(OAuthProvider, OAuth);
extension_plugin!(IntakeParser, Intake);
extension_plugin!(MetadataEnricher, Enricher);
extension_plugin!(NotifierPlugin, Notifier);
extension_plugin!(PostprocessPlugin, Postprocess);
extension_plugin!(StoragePlugin, Storage);

impl ClaimingPlugin for AuthProvider {
    const KIND: &'static str = "authentication";
}

impl ClaimingPlugin for OAuthProvider {
    const KIND: &'static str = "oauth";
}

/// One installed plugin with the manifest it was built from.
pub(crate) struct Installed<P> {
    pub(crate) manifest: PluginManifest,
    pub(crate) plugin: P,
}

/// The installed plugins of one type, newest version of each, in the registry's order.
pub struct PluginSet<P> {
    pub(crate) plugins: Vec<Installed<P>>,
}

/// Every installed package of `P`'s type, each version, newest first; a package that fails to
/// build is logged and skipped.
fn every_version<P: ExtensionPlugin>(
    registry: &PluginTypeRegistry,
    host: Option<Arc<dyn ResolverHost>>,
) -> Vec<Installed<P>> {
    registry.instantiate(&P::PLUGIN_TYPE, |package| {
        P::build(package, host.clone()).map(|plugin| Installed {
            manifest: package.manifest.clone(),
            plugin,
        })
    })
}

impl<P: ExtensionPlugin> PluginSet<P> {
    /// Loads every installed plugin of this type, skipping any that fails to build.
    ///
    /// A broken plugin costs its own feature and nothing else, and the failure is logged
    /// rather than taking down what the plugin type plugs into.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    ///
    /// The registry hands out one package per installed *version*, newest first, so a machine
    /// that still has 1.2.3 next to 1.2.4 of one plugin would ask both: the same link came back
    /// with every field twice. The first entry for an id wins, which is the highest SemVer, and
    /// the copies behind it are left unused on disk.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let mut plugins = every_version::<P>(registry, host);
        let mut seen = HashSet::new();
        plugins.retain(|installed| seen.insert(installed.manifest.id.to_string()));
        Self { plugins }
    }
}

impl<P> PluginSet<P> {
    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Whether any plugin of this type is installed at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Whether a plugin id names an installed plugin, for validating a saved reference.
    #[must_use]
    pub fn contains(&self, plugin_id: &str) -> bool {
        self.get(plugin_id).is_some()
    }

    /// The installed plugin with this id.
    pub(crate) fn get(&self, plugin_id: &str) -> Option<&Installed<P>> {
        self.plugins
            .iter()
            .find(|installed| installed.manifest.id.to_string() == plugin_id)
    }

    /// Every installed plugin, newest version of each, in the registry's order.
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, Installed<P>> {
        self.plugins.iter()
    }

    /// Every installed plugin, sorted by name so a list built from it does not reshuffle
    /// itself.
    pub(crate) fn by_name(&self) -> Vec<&Installed<P>> {
        let mut listed: Vec<&Installed<P>> = self.plugins.iter().collect();
        listed.sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
        listed
    }
}

/// The installed plugins that sign in to a provider, one per claimed provider slug.
pub struct ClaimedPlugins<P> {
    /// Shared, so a set with a first install joined to it (RD-170-12) is built without
    /// compiling what is already running a second time.
    plugins: Vec<Arc<Installed<P>>>,
    /// Provider slug the plugin claims — the same key resolver dispatch uses — to its index.
    /// A slug rather than a plugin id, because that is what an account carries.
    by_slug: HashMap<String, usize>,
}

impl<P: ClaimingPlugin> ClaimedPlugins<P> {
    /// Loads every installed provider of this type, skipping any that fails to build.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let mut plugins = Vec::new();
        let mut by_slug = HashMap::new();
        for provider in every_version::<P>(registry, host) {
            let claims = provider
                .manifest
                .extension
                .as_ref()
                .map(|extension| extension.claims.clone())
                .unwrap_or_default();
            if claims.is_empty() {
                // A provider slug is what decides which plugin signs an account in. One that
                // claims nothing names no account it could run for, so it is left out rather
                // than offered for everything.
                tracing::warn!(
                    plugin = %provider.manifest.name,
                    "{} plugin claims no provider and cannot be used",
                    P::KIND
                );
                continue;
            }
            let index = plugins.len();
            plugins.push(Arc::new(provider));
            for slug in claims {
                // The newest version of each plugin comes first, and the first claim of a
                // slug wins: two plugins claiming one provider is a conflict, not a chain.
                by_slug.entry(slug.to_ascii_lowercase()).or_insert(index);
            }
        }
        Self { plugins, by_slug }
    }
}

impl<P> ClaimedPlugins<P> {
    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
            by_slug: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Whether any version of this plugin is in the set.
    #[must_use]
    pub fn has_plugin(&self, id: rd_core::PluginId) -> bool {
        self.plugins
            .iter()
            .any(|provider| provider.manifest.id == id)
    }

    /// This set with the plugins of `addition` joined to it: a first install that runs without
    /// a restart (RD-170-12). A slug already claimed stays with its plugin, as the first claim
    /// wins at a start.
    #[must_use]
    pub fn joined(&self, addition: Self) -> Self {
        let mut plugins = self.plugins.clone();
        let mut by_slug = self.by_slug.clone();
        let offset = plugins.len();
        plugins.extend(addition.plugins);
        for (slug, index) in addition.by_slug {
            by_slug.entry(slug).or_insert(offset + index);
        }
        Self { plugins, by_slug }
    }

    /// Whether a provider can be signed in to by a plugin rather than by typing a key.
    #[must_use]
    pub fn supports(&self, provider_slug: &str) -> bool {
        self.by_slug
            .contains_key(&provider_slug.to_ascii_lowercase())
    }

    /// The plugin id that would run a flow for this provider.
    #[must_use]
    pub fn plugin_id(&self, provider_slug: &str) -> Option<String> {
        self.provider(provider_slug)
            .ok()
            .map(|provider| provider.manifest.id.to_string())
    }

    /// The plugin that claims this provider, or the reason there is none.
    ///
    /// The error is typed rather than prose because the sweeps that call this have to tell a
    /// provider nobody claims -- which waiting never fixes -- from a call that failed.
    pub(crate) fn provider(&self, provider_slug: &str) -> ProviderResult<&Installed<P>> {
        self.by_slug
            .get(&provider_slug.to_ascii_lowercase())
            .and_then(|index| self.plugins.get(*index))
            .map(Arc::as_ref)
            .ok_or_else(|| ProviderError::no_plugin(provider_slug))
    }
}
