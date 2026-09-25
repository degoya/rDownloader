//! Notification destination plugins (RD-090-15).
//!
//! A plugin delivers; it does not decide whether to try again. Retries, backoff and quiet
//! hours stay with the notification hub, which already owns them for the built-in webhook,
//! SMTP and Apprise targets — a destination that could set its own retry policy would be a
//! way for one plugin to keep the queue busy for everyone.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry, extension::NotifierPlugin,
};

/// The installed notification destinations, newest version of each.
pub struct NotifierPlugins {
    /// Keyed by plugin id, which is what a notification target stores.
    plugins: HashMap<String, Destination>,
}

struct Destination {
    manifest: PluginManifest,
    plugin: NotifierPlugin,
}

/// What a destination looks like to whoever is choosing one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DestinationInfo {
    pub plugin_id: String,
    /// Default display name; the localised one comes from the plugin's own locale bundle.
    pub name: String,
    pub version: String,
}

impl NotifierPlugins {
    /// Loads every installed notification destination, skipping any that fails to build.
    ///
    /// A broken plugin costs its own targets and nothing else: the built-in kinds keep
    /// delivering, and the failure is logged rather than taking the hub down with it.
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
        let loaded = registry.instantiate(&PluginType::Notifier, |package| {
            NotifierPlugin::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| Destination {
                    manifest: package.manifest.clone(),
                    plugin,
                },
            )
        });
        // The registry yields the newest version of each plugin first, so the first entry
        // for an id wins and an older version left on disk is ignored.
        let mut plugins = HashMap::new();
        for destination in loaded {
            plugins
                .entry(destination.manifest.id.to_string())
                .or_insert(destination);
        }
        Self { plugins }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Whether a plugin id names an installed destination, for validating a saved target.
    #[must_use]
    pub fn contains(&self, plugin_id: &str) -> bool {
        self.plugins.contains_key(plugin_id)
    }

    /// Whether a destination would be refused at delivery (RD-130-15), for refusing it when
    /// the target is saved. `None` means the plugin is not installed.
    #[must_use]
    pub fn check_destination(
        &self,
        plugin_id: &str,
        destination: &str,
    ) -> Option<Result<(), rd_core::Failure>> {
        Some(
            self.plugins
                .get(plugin_id)?
                .plugin
                .check_destination(destination),
        )
    }

    /// Every installed destination, sorted by name so the list does not reshuffle itself.
    #[must_use]
    pub fn list(&self) -> Vec<DestinationInfo> {
        let mut destinations: Vec<DestinationInfo> = self
            .plugins
            .values()
            .map(|destination| DestinationInfo {
                plugin_id: destination.manifest.id.to_string(),
                name: destination.manifest.name.clone(),
                version: destination.manifest.version.clone(),
            })
            .collect();
        destinations.sort_by(|left, right| left.name.cmp(&right.name));
        destinations
    }

    /// Delivers one message through one plugin.
    ///
    /// `Ok(None)` means the plugin is not installed — a target naming a plugin that has been
    /// removed, which the caller reports as a permanent failure rather than retrying forever.
    pub async fn deliver(
        &self,
        plugin_id: &str,
        message: rd_plugin_host::extension::Delivery<'_>,
    ) -> Option<Result<()>> {
        let destination = self.plugins.get(plugin_id)?;
        Some(destination.plugin.deliver(message).await)
    }
}
