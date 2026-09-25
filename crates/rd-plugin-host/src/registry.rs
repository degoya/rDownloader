//! Per-type instantiation of installed plugin packages.
//!
//! Verification already decides whether a package may run at all; this decides *what* runs
//! it. Packages are bucketed by `plugin_type` and each bucket is instantiated on its own, so
//! a component that fails to compile costs its own plugin and nothing else — not the other
//! plugins of its type, not the plugins of another type, and not the core services that ask
//! for them.

use std::sync::Arc;

use crate::{PluginInstaller, PluginType, VerifiedPackage};

/// The installed packages of one core, grouped by what they are.
pub struct PluginTypeRegistry {
    packages: Vec<Arc<VerifiedPackage>>,
}

impl PluginTypeRegistry {
    /// Re-verifies and loads every installed package once, for all the adapters to share.
    ///
    /// Each adapter used to call `load_verified` for itself, and one such call is an Ed25519
    /// check, a wasmparser validation, a sandbox engine — with the epoch ticker thread it
    /// spawns and joins — and a compile for *every* installed package, not only the type that
    /// adapter wants. Eleven adapters against thirty installed plugins is three hundred of
    /// those on a machine this project already treats as memory-constrained. Build one
    /// registry per start, hand it to each adapter, and drop it once they are built: it holds
    /// every component's bytes for as long as it lives.
    pub async fn load(installer: &PluginInstaller) -> anyhow::Result<Self> {
        Ok(Self::new(installer.load_verified().await?))
    }

    /// Buckets verified packages; the input order (newest version first) is preserved.
    #[must_use]
    pub fn new(packages: Vec<VerifiedPackage>) -> Self {
        Self {
            packages: packages.into_iter().map(Arc::new).collect(),
        }
    }

    /// Packages declaring one type, newest version of each plugin first.
    pub fn of_type(&self, plugin_type: &PluginType) -> impl Iterator<Item = &Arc<VerifiedPackage>> {
        self.packages
            .iter()
            .filter(move |package| &package.manifest.plugin_type == plugin_type)
    }

    /// Instantiates every package of one type, keeping the failures out of the result.
    ///
    /// `build` runs once per package. An error is logged against that plugin and skipped;
    /// the remaining packages are unaffected, which is the whole point of the split.
    pub fn instantiate<T>(
        &self,
        plugin_type: &PluginType,
        build: impl Fn(&VerifiedPackage) -> anyhow::Result<T>,
    ) -> Vec<T> {
        let mut loaded = Vec::new();
        for package in self.of_type(plugin_type) {
            match build(package) {
                Ok(instance) => loaded.push(instance),
                Err(error) => tracing::warn!(
                    plugin = %package.manifest.name,
                    plugin_id = %package.manifest.id,
                    version = %package.manifest.version,
                    plugin_type = plugin_type.as_str(),
                    error = %error,
                    "skipping installed plugin package that failed to load"
                ),
            }
        }
        loaded
    }
}
