//! Per-type instantiation of installed plugin packages.
//!
//! Verification already decides whether a package may run at all; this decides *what* runs
//! it. Packages are bucketed by `plugin_type` and each bucket is instantiated on its own, so
//! a component that fails to compile costs its own plugin and nothing else — not the other
//! plugins of its type, not the plugins of another type, and not the core services that ask
//! for them.

use std::sync::Arc;

use crate::{PluginInstaller, PluginType, VerifiedPackage, VersionRole};

/// The installed packages of one core, grouped by what they are.
pub struct PluginTypeRegistry {
    packages: Vec<(Arc<VerifiedPackage>, VersionRole)>,
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
        Ok(Self::with_roles(
            installer.load_verified_with_roles().await?,
        ))
    }

    /// Buckets verified packages; the input order (newest version first) is preserved.
    ///
    /// Without a version choice the first package of an id is its default and the rest are
    /// retained, which is exactly what the installer decides for a plugin nobody chose for.
    #[must_use]
    pub fn new(packages: Vec<VerifiedPackage>) -> Self {
        let mut seen = std::collections::HashSet::new();
        Self {
            packages: packages
                .into_iter()
                .map(|package| {
                    let role = if seen.insert(package.manifest.id) {
                        VersionRole::Default
                    } else {
                        VersionRole::Retained
                    };
                    (Arc::new(package), role)
                })
                .collect(),
        }
    }

    /// Buckets packages whose role the installer already decided (RD-140-02).
    #[must_use]
    pub fn with_roles(packages: Vec<(VerifiedPackage, VersionRole)>) -> Self {
        Self {
            packages: packages
                .into_iter()
                .map(|(package, role)| (Arc::new(package), role))
                .collect(),
        }
    }

    /// Packages declaring one type, the default version of each plugin first.
    ///
    /// A staged version is left out: every consumer of this keeps the first entry per id or
    /// looks up an exact pinned version of a running job, and neither may reach a version that
    /// is only under test. Resolvers, whose jobs can be pinned to it on purpose, ask
    /// [`Self::instantiate_with_roles`] instead.
    #[cfg(test)]
    pub fn of_type(&self, plugin_type: &PluginType) -> impl Iterator<Item = &Arc<VerifiedPackage>> {
        self.packages
            .iter()
            .filter(move |(package, role)| {
                &package.manifest.plugin_type == plugin_type && *role != VersionRole::Staged
            })
            .map(|(package, _)| package)
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
        build_each(
            self.packages.iter().filter(|(package, role)| {
                &package.manifest.plugin_type == plugin_type && *role != VersionRole::Staged
            }),
            plugin_type,
            build,
        )
        .into_iter()
        .map(|(instance, _)| instance)
        .collect()
    }

    /// The same, staged versions included, each instance with the role of its version.
    ///
    /// For the resolver chain only: a download pinned to a staged version has to find it, and
    /// the role is what keeps every unpinned lookup on the default version.
    pub fn instantiate_with_roles<T>(
        &self,
        plugin_type: &PluginType,
        build: impl Fn(&VerifiedPackage) -> anyhow::Result<T>,
    ) -> Vec<(T, VersionRole)> {
        build_each(
            self.packages
                .iter()
                .filter(|(package, _)| &package.manifest.plugin_type == plugin_type),
            plugin_type,
            build,
        )
    }
}

/// Builds each package, logging a failure against that plugin and skipping it.
fn build_each<'a, T>(
    packages: impl Iterator<Item = &'a (Arc<VerifiedPackage>, VersionRole)>,
    plugin_type: &PluginType,
    build: impl Fn(&VerifiedPackage) -> anyhow::Result<T>,
) -> Vec<(T, VersionRole)> {
    let mut loaded = Vec::new();
    for (package, role) in packages {
        match build(package) {
            Ok(instance) => loaded.push((instance, *role)),
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
