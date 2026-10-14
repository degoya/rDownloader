//! The start's pass over the installed versions (RD-1291-03): removes the versions this build
//! can no longer load while a loadable version of the same plugin stands in for them, and
//! reports every other version that does not verify once, one warning per plugin.
//!
//! Installing never removes an older version, and after a contract change every version built
//! against the old one is refused for good: nothing loads it, and a rollback (RD-140-02) cannot
//! choose it, because a choice only ever points at a version that loaded. Such a version is
//! removed only when its plugin keeps a version that loads and the operator has not chosen it
//! as active or staged. A version refused for any other reason -- a withdrawn key or digest, a
//! file changed on disk, a contract newer than this build's -- stays: deleting it would hide
//! exactly what someone has to look at.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::Deserialize;

use super::disk::{load_one_contents, read_optional_bounded, version_directories};
use crate::{
    MANIFEST_VERSION, MAX_MANIFEST_BYTES, PluginInstaller, PluginVerifier, SUPPORTED_API_VERSIONS,
};

/// What the start's sweep did.
#[derive(Clone, Debug, Default)]
pub struct VersionSweep {
    /// `(plugin id, version)` of every version removed.
    pub removed: Vec<(String, String)>,
    /// Every plugin a warning was written for, once each.
    pub warned: Vec<UnloadableVersions>,
}

/// The versions of one plugin that stay installed although they do not verify.
#[derive(Clone, Debug)]
pub struct UnloadableVersions {
    pub id: String,
    /// The manifest's name where one still reads, otherwise the id.
    pub name: String,
    /// Whether another version of the plugin loads; without one the plugin does not run at all.
    pub loadable: bool,
    /// The versions kept, lowest first.
    pub versions: Vec<String>,
}

/// One installed version that did not verify, and why.
struct Refused {
    version: String,
    path: PathBuf,
    reason: String,
}

/// The installed versions of one plugin id, sorted by what the sweep does with them.
#[derive(Default)]
struct PluginVersions {
    name: Option<String>,
    loadable: Vec<String>,
    /// Written for a contract older than every one this build links.
    outdated: Vec<Refused>,
    /// Refused for any other reason.
    other: Vec<Refused>,
}

/// The fields that tell an outdated package from a broken one, in every manifest revision.
#[derive(Deserialize)]
struct ContractHeader {
    manifest_version: u32,
    name: Option<String>,
    api_version: Option<String>,
}

impl ContractHeader {
    /// An older manifest revision, or this revision with an older `api_version` than every one
    /// in `SUPPORTED_API_VERSIONS`. A newer one is a package that waits for a newer build.
    fn outdated(&self) -> bool {
        if self.manifest_version != MANIFEST_VERSION {
            return self.manifest_version < MANIFEST_VERSION;
        }
        let Some(api) = self
            .api_version
            .as_deref()
            .and_then(|value| semver::Version::parse(value).ok())
        else {
            return false;
        };
        SUPPORTED_API_VERSIONS
            .iter()
            .filter_map(|value| semver::Version::parse(value).ok())
            .min()
            .is_some_and(|oldest| api < oldest)
    }
}

impl PluginInstaller {
    /// Removes the versions this build can no longer load where a loadable version of the same
    /// plugin is installed, and warns once per plugin about every version it keeps although it
    /// does not verify. Called once at start, after the bundled packages are synced -- the
    /// bundle is what brings the loadable version after a contract change -- and before
    /// anything loads a plugin. Never fails the start.
    pub async fn sweep_unloadable_versions(&self) -> VersionSweep {
        let root = self.root.clone();
        let verifier = self.verifier.clone();
        let classified = tokio::task::spawn_blocking(move || classify(&root, &verifier))
            .await
            .map_err(anyhow::Error::from)
            .and_then(std::convert::identity);
        let plugins = match classified {
            Ok(plugins) => plugins,
            Err(error) => {
                tracing::warn!(%error, "installed plugin versions could not be checked");
                return VersionSweep::default();
            }
        };
        let choices = self.version_choices();
        let mut sweep = VersionSweep::default();
        for (id, plugin) in plugins {
            let PluginVersions {
                name,
                loadable,
                outdated,
                other: mut kept,
            } = plugin;
            if loadable.is_empty() {
                kept.extend(outdated);
            } else {
                // The version that runs is one that loaded, so it is never among these; a
                // chosen active or staged version is the operator's to remove.
                let choice = choices.get(&id);
                for refused in outdated {
                    let chosen = choice.is_some_and(|choice| {
                        [&choice.active, &choice.staged]
                            .into_iter()
                            .any(|version| version.as_deref() == Some(refused.version.as_str()))
                    });
                    if chosen {
                        kept.push(refused);
                        continue;
                    }
                    match self.remove_installed(&refused.path).await {
                        Ok(()) => sweep.removed.push((id.clone(), refused.version)),
                        Err(error) => kept.push(Refused {
                            reason: format!("{error:#}"),
                            ..refused
                        }),
                    }
                }
            }
            if kept.is_empty() {
                continue;
            }
            kept.sort_by(|left, right| {
                semver::Version::parse(&left.version)
                    .ok()
                    .cmp(&semver::Version::parse(&right.version).ok())
            });
            for refused in &kept {
                self.reported.first(&refused.path);
            }
            let name = name.unwrap_or_else(|| id.clone());
            let versions: Vec<String> =
                kept.iter().map(|refused| refused.version.clone()).collect();
            let reasons = kept
                .iter()
                .map(|refused| format!("{}: {}", refused.version, refused.reason))
                .collect::<Vec<_>>()
                .join("; ");
            if loadable.is_empty() {
                tracing::warn!(
                    plugin_id = %id,
                    plugin = %name,
                    versions = %versions.join(", "),
                    reasons = %reasons,
                    "installed plugin has no version this build can load"
                );
            } else {
                tracing::warn!(
                    plugin_id = %id,
                    plugin = %name,
                    versions = %versions.join(", "),
                    reasons = %reasons,
                    "installed plugin versions that no longer verify are kept"
                );
            }
            sweep.warned.push(UnloadableVersions {
                id,
                name,
                loadable: !loadable.is_empty(),
                versions,
            });
        }
        if !sweep.removed.is_empty() {
            tracing::info!(
                removed = sweep.removed.len(),
                "removed installed plugin versions this build can no longer load"
            );
        }
        sweep
    }
}

/// Every installed version, verified as the provider rows read it and grouped by plugin id.
fn classify(root: &Path, verifier: &PluginVerifier) -> Result<BTreeMap<String, PluginVersions>> {
    let _batch = crate::unsigned_notice::UnsignedBatch::open();
    let mut plugins: BTreeMap<String, PluginVersions> = BTreeMap::new();
    for path in version_directories(root)? {
        let (Some(id), Some(version)) = (segment(path.parent()), segment(Some(&path))) else {
            continue;
        };
        let plugin = plugins.entry(id).or_default();
        match load_one_contents(&path, verifier) {
            Ok(package) => {
                plugin.name = Some(package.manifest.name);
                plugin.loadable.push(version);
            }
            Err(error) => {
                let header = read_optional_bounded(&path.join("manifest.toml"), MAX_MANIFEST_BYTES)
                    .ok()
                    .flatten()
                    .and_then(|bytes| toml::from_slice::<ContractHeader>(&bytes).ok());
                if plugin.name.is_none() {
                    plugin.name = header.as_ref().and_then(|header| header.name.clone());
                }
                let refused = Refused {
                    version,
                    path,
                    reason: format!("{error:#}"),
                };
                if header.as_ref().is_some_and(ContractHeader::outdated) {
                    plugin.outdated.push(refused);
                } else {
                    plugin.other.push(refused);
                }
            }
        }
    }
    Ok(plugins)
}

/// A directory's own name: the plugin id one level up, the version at the bottom.
fn segment(path: Option<&Path>) -> Option<String> {
    path.and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "sweep_tests.rs"]
mod tests;
