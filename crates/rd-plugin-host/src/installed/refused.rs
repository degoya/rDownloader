//! Installed versions this build refuses but that still name themselves: what a newer bundled
//! or repository package replaces.
//!
//! A release that raises the `rdownloader:plugin` contract refuses every package built against
//! the old one. Those packages are not in `list_installed`, so the start's bundled sync took
//! each such plugin for one that was never installed and left it alone, and the repository
//! never offered it an update: after the self-update to 1.25.0 every plugin of an installation
//! stayed refused although the update carried a runnable version of each (RD-1251-01).
//! Counting them here as installed is what lets the bundle and the repository replace them.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::PluginId;

use super::disk::{read_optional_bounded, version_directories};
use crate::{
    MAX_MANIFEST_BYTES, ManifestHeader, PluginInstaller, PluginManifest,
    manifest::validate_manifest,
};

/// One installed version this build refuses, as far as its manifest still says what it is.
#[derive(Clone, Debug)]
pub struct RefusedInstall {
    pub id: PluginId,
    pub version: String,
    /// The whole manifest when it still reads as this revision: the key and the permissions an
    /// update is compared with. `None` for a package of another manifest revision, which only
    /// the bundle replaces, since nothing proves which publisher it came from.
    pub manifest: Option<PluginManifest>,
    path: PathBuf,
}

impl PluginInstaller {
    /// Every installed version this build refuses whose manifest still names its id and version.
    pub(crate) async fn list_refused(&self) -> Result<Vec<RefusedInstall>> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || list_refused_sync(&root)).await?
    }

    /// The installed manifests an update is measured against: the ones this build runs and the
    /// refused ones that still read as this manifest revision. Unverified, like
    /// `list_installed`; only versions and keys are read from it.
    pub(crate) async fn list_update_bases(&self) -> Result<Vec<PluginManifest>> {
        let mut manifests = self.list_installed().await?;
        manifests.extend(
            self.list_refused()
                .await?
                .into_iter()
                .filter_map(|refused| refused.manifest),
        );
        Ok(manifests)
    }

    /// Id and version of every installed version, runnable or refused: what the bundled sync
    /// compares its packages with.
    pub(crate) async fn installed_or_refused_versions(&self) -> Result<Vec<(PluginId, String)>> {
        let mut versions: Vec<(PluginId, String)> = self
            .list_installed()
            .await?
            .into_iter()
            .map(|manifest| (manifest.id, manifest.version))
            .collect();
        versions.extend(
            self.list_refused()
                .await?
                .into_iter()
                .map(|refused| (refused.id, refused.version)),
        );
        Ok(versions)
    }

    /// Removes the refused versions of `id` older than `version`, which was just installed;
    /// returns the versions removed.
    ///
    /// A refused version runs nowhere and is no version to go back to, and the newer one now
    /// stands in for it, so keeping it would only leave the plugin listed as refused. A refused
    /// version *newer* than the install -- one that needs a newer rDownloader -- stays. A
    /// failure is logged and never fails the install that came before it.
    pub(crate) async fn remove_superseded_refused(
        &self,
        id: PluginId,
        version: &str,
    ) -> Vec<String> {
        let Ok(installed) = semver::Version::parse(version) else {
            return Vec::new();
        };
        let refused = match self.list_refused().await {
            Ok(refused) => refused,
            Err(error) => {
                tracing::warn!(%error, "refused plugin versions could not be listed");
                return Vec::new();
            }
        };
        let mut removed = Vec::new();
        for old in refused.into_iter().filter(|old| old.id == id) {
            if semver::Version::parse(&old.version).is_ok_and(|old| old < installed) {
                match self.remove_installed(&old.path).await {
                    Ok(()) => removed.push(old.version),
                    Err(error) => tracing::warn!(
                        %error,
                        plugin_id = %id,
                        version = %old.version,
                        "a refused plugin version could not be removed after its update"
                    ),
                }
            }
        }
        removed
    }
}

fn list_refused_sync(root: &Path) -> Result<Vec<RefusedInstall>> {
    let mut refused = Vec::new();
    for version in version_directories(root)? {
        let Some(bytes) =
            read_optional_bounded(&version.join("manifest.toml"), MAX_MANIFEST_BYTES)?
        else {
            continue;
        };
        let Ok(header) = toml::from_slice::<ManifestHeader>(&bytes) else {
            continue;
        };
        let manifest = toml::from_slice::<PluginManifest>(&bytes).ok();
        if manifest
            .as_ref()
            .is_some_and(|manifest| validate_manifest(manifest).is_ok())
        {
            continue;
        }
        refused.push(RefusedInstall {
            id: header.id,
            version: header.version,
            manifest,
            path: version,
        });
    }
    Ok(refused)
}
