//! `PluginInstaller` and the install itself: a verified package is written to a staging
//! directory beside the version directories and renamed into place, so a stop half-way never
//! leaves a version that loads. What is installed is listed and loaded in `installed.rs`.
//!
//! Split out of `lib.rs` (PLUG-21).

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::{
    BundledPackage, COMPONENT_MEMBER, INSTALL_STAGING_PREFIX, InstalledPackage, MANIFEST_MEMBER,
    PluginVerifier, SIGNATURE_MEMBER, VerifiedPackage, VerifyError, VersionChoices,
    manifest::safe_segment,
};

/// Atomically installs packages into `<root>/<plugin-id>/<version>`.
#[derive(Clone)]
pub struct PluginInstaller {
    pub(crate) root: PathBuf,
    pub(crate) verifier: PluginVerifier,
    /// Plugin ids the user switched off. Shared with every clone, because the installer is
    /// cloned into each subsystem that loads plugins.
    pub(crate) disabled: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    /// The operator's version choices as read at start (RD-140-02), shared like `disabled`.
    pub(crate) choices: std::sync::Arc<std::sync::RwLock<VersionChoices>>,
    /// The versions installed when this start loaded its plugins (RD-160-09), shared the same
    /// way; `None` until the start records them.
    pub(crate) started: std::sync::Arc<std::sync::RwLock<Option<StartedVersions>>>,
    /// The packages shipped next to the executable, as the start-up sync verified them
    /// (RD-160-05). What the wizard and the plugin manager offer as "available".
    pub(crate) bundled: std::sync::Arc<std::sync::RwLock<Vec<BundledPackage>>>,
}

/// Installed versions by plugin id.
pub type StartedVersions = std::collections::BTreeMap<String, Vec<String>>;

impl PluginInstaller {
    #[must_use]
    pub fn new(root: PathBuf, verifier: PluginVerifier) -> Self {
        Self {
            root,
            verifier,
            disabled: std::sync::Arc::default(),
            choices: std::sync::Arc::default(),
            started: std::sync::Arc::default(),
            bundled: std::sync::Arc::default(),
        }
    }

    /// The directory installed plugins live in; a storage root may not reach it.
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Replaces the active and staged version of every plugin that has a choice.
    ///
    /// Seeded once at start, before anything loads a plugin, and deliberately not touched when
    /// the operator changes a choice later: the running resolvers and adapters were built from
    /// the choice of this start, and this is what says which one that was. A new choice takes
    /// effect at the next start, like an install.
    pub fn set_version_choices(&self, choices: VersionChoices) {
        if let Ok(mut current) = self.choices.write() {
            *current = choices;
        }
    }

    /// The version choices this start loaded with.
    #[must_use]
    pub fn version_choices(&self) -> VersionChoices {
        self.choices
            .read()
            .map(|choices| choices.clone())
            .unwrap_or_default()
    }

    /// Records the versions installed now as the ones this start loads (RD-160-09).
    ///
    /// Called once at start, after the bundled packages are synced and before anything loads a
    /// plugin. A version installed later lies on disk, and nothing runs it before the next
    /// start: resolvers and adapters are built once per start. The one exception is a first
    /// install that joins the running service, which [`Self::record_started_version`] adds
    /// (RD-170-12). Without this record the newest version on disk -- the one just installed --
    /// passed for the one that runs.
    pub async fn record_started_versions(&self) -> Result<()> {
        let mut versions = StartedVersions::new();
        for manifest in self.list_installed().await? {
            versions
                .entry(manifest.id.to_string())
                .or_default()
                .push(manifest.version);
        }
        if let Ok(mut started) = self.started.write() {
            *started = Some(versions);
        }
        Ok(())
    }

    /// Counts one version as loaded by this start: a first install that joined the running
    /// service instead of waiting for the next start (RD-170-12). Without a record of the start
    /// every installed version already counts, so there is nothing to add to.
    pub fn record_started_version(&self, id: &str, version: &str) {
        if let Ok(mut started) = self.started.write()
            && let Some(started) = started.as_mut()
        {
            let versions = started.entry(id.to_owned()).or_default();
            if !versions.iter().any(|known| known == version) {
                versions.push(version.to_owned());
            }
        }
    }

    /// The versions installed when this start loaded its plugins; `None` when the start did not
    /// record them, as in a test that builds the service without starting it.
    #[must_use]
    pub fn started_versions(&self) -> Option<StartedVersions> {
        self.started.read().ok().and_then(|started| started.clone())
    }

    /// Replaces the set of switched-off plugins.
    ///
    /// Only `load_verified` honours it, so a disabled plugin is never compiled or executed while
    /// still being listed by the API — otherwise it could not be switched back on. Like an
    /// install, this takes full effect on the next start: resolvers and extension hosts are
    /// built when their subsystem starts.
    pub fn set_disabled(&self, ids: impl IntoIterator<Item = String>) {
        if let Ok(mut disabled) = self.disabled.write() {
            *disabled = ids.into_iter().collect();
        }
    }

    /// Whether this plugin id is switched off.
    #[must_use]
    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled
            .read()
            .is_ok_and(|disabled| disabled.contains(id))
    }

    /// The trust store shared with every clone of this installer.
    #[must_use]
    pub fn verifier(&self) -> &PluginVerifier {
        &self.verifier
    }

    /// Verifies and installs a package without replacing an active version in place.
    pub async fn install(&self, package_path: PathBuf) -> Result<InstalledPackage, VerifyError> {
        let verifier = self.verifier.clone();
        let package = tokio::task::spawn_blocking(move || verifier.verify_file(&package_path))
            .await
            .map_err(|error| VerifyError::Other(error.into()))??;
        Ok(self.install_verified(package).await?)
    }

    /// Verifies and installs package bytes without creating an untrusted upload file.
    pub async fn install_bytes(&self, bytes: Vec<u8>) -> Result<InstalledPackage, VerifyError> {
        let verifier = self.verifier.clone();
        let package = tokio::task::spawn_blocking(move || verifier.verify_bytes(&bytes))
            .await
            .map_err(|error| VerifyError::Other(error.into()))??;
        Ok(self.install_verified(package).await?)
    }

    /// Removes one installed version directory, used to roll back an install the caller
    /// could not accept. Refuses paths outside this installer's root.
    pub async fn remove_installed(&self, path: &Path) -> Result<()> {
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !target.starts_with(&root) || target == root {
            bail!(
                "refusing to remove {} outside the plugin root",
                path.display()
            );
        }
        tokio::fs::remove_dir_all(&target).await?;
        // Drop the plugin directory too when that version was its only one.
        if let Some(parent) = target.parent()
            && parent != root
            && let Ok(mut entries) = tokio::fs::read_dir(parent).await
            && entries.next_entry().await.ok().flatten().is_none()
        {
            let _ = tokio::fs::remove_dir(parent).await;
        }
        Ok(())
    }

    pub(crate) async fn install_verified(
        &self,
        package: VerifiedPackage,
    ) -> Result<InstalledPackage> {
        let version = safe_segment(&package.manifest.version)?;
        let plugin_root = self.root.join(package.manifest.id.to_string());
        let destination = plugin_root.join(version);
        if destination.exists() {
            bail!("plugin version is already installed");
        }
        tokio::fs::create_dir_all(&plugin_root).await?;
        let staging = plugin_root.join(format!(
            "{INSTALL_STAGING_PREFIX}{}",
            rd_core::PluginId::new()
        ));
        tokio::fs::create_dir(&staging).await?;
        if let Err(error) = write_staging(&staging, &package).await {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(error);
        }
        // A stop here leaves the whole package under its staging name: never loaded (see
        // `installed::version_directories`), removed by the next start (RD-170-07).
        rd_core::failpoint!("plugin.before_version_promoted", || anyhow::anyhow!(
            "crash point"
        ));
        if let Err(error) = tokio::fs::rename(&staging, &destination).await {
            // `version_directories` enumerates every directory under the plugin root, so a
            // leftover `.install-<id>` is loaded and listed as a second copy of the plugin
            // that `remove_version` cannot address.
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(error.into());
        }
        Ok(InstalledPackage {
            path: destination,
            manifest: package.manifest,
        })
    }
}

async fn write_staging(staging: &Path, package: &VerifiedPackage) -> Result<()> {
    tokio::fs::write(staging.join(MANIFEST_MEMBER), &package.manifest_bytes).await?;
    tokio::fs::write(staging.join(COMPONENT_MEMBER), &package.component).await?;
    if let Some(signature) = &package.signature {
        tokio::fs::write(staging.join(SIGNATURE_MEMBER), signature).await?;
    }
    if !package.locales.is_empty() {
        let directory = staging.join("locales");
        tokio::fs::create_dir(&directory).await?;
        for (language, bytes) in &package.locales {
            tokio::fs::write(directory.join(format!("{language}.json")), bytes).await?;
        }
    }
    Ok(())
}
