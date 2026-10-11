//! Discovery and re-verification of versioned installed plugin packages.

use anyhow::Result;

use crate::{PluginInstaller, PluginManifest, VerifiedPackage, VersionRole};

mod disk;

use disk::{
    install_stagings, list_incompatible_sync, list_installed_sync, load_one, load_verified_sync,
    locale_bundle_sync, read_package_parts, verified_contents_sync,
};

/// An installed package this build refuses, kept listable so it can be recognised and removed.
#[derive(Clone, Debug)]
pub struct IncompatiblePlugin {
    /// Plugin id when the manifest is readable at all, otherwise the directory name.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Stable code the UI translates: `plugin.manifest_outdated`, `plugin.capability_unknown`
    /// or `plugin.manifest_unreadable`.
    pub code: String,
}

impl PluginInstaller {
    /// Removes what installs that stopped before their rename left behind; returns how many.
    /// Called once at start, before anything installs: a staging directory is never the only
    /// copy of anything, the package it holds is fetched again by the next update pass.
    pub async fn sweep_install_staging(&self) -> usize {
        let root = self.root.clone();
        let listed = tokio::task::spawn_blocking(move || install_stagings(&root))
            .await
            .map_err(anyhow::Error::from)
            .and_then(std::convert::identity);
        let stagings = match listed {
            Ok(stagings) => stagings,
            Err(error) => {
                tracing::warn!(%error, "leftover plugin install folders could not be listed");
                return 0;
            }
        };
        let mut removed = 0;
        for staging in stagings {
            match tokio::fs::remove_dir_all(&staging).await {
                Ok(()) => removed += 1,
                Err(error) => tracing::warn!(
                    %error,
                    path = %staging.display(),
                    "a leftover plugin install folder could not be removed"
                ),
            }
        }
        removed
    }

    /// Reads all valid installed manifests in stable display order.
    ///
    /// Parsed straight off disk and deliberately unverified, because the Plugins view has to
    /// name a package whose signature no longer checks out so it can be removed. Nothing that
    /// *acts* on a manifest may be built from this: the provider rows and the locale bundle go
    /// through `verified_contents`, which is what makes an edited manifest stop counting.
    pub async fn list_installed(&self) -> Result<Vec<PluginManifest>> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || list_installed_sync(&root)).await?
    }

    /// Installed packages this build cannot run, in the same display order.
    ///
    /// These are skipped everywhere else — startup, locale bundles, resolver loading — so a
    /// broken or outdated package disables itself and nothing more. Listing them separately
    /// is what turns that silence into something a user can act on.
    pub async fn list_incompatible(&self) -> Result<Vec<IncompatiblePlugin>> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || list_incompatible_sync(&root)).await?
    }

    /// Removes one installed plugin version by id and version.
    ///
    /// Returns whether anything was there to remove, so a repeated request is not an error.
    pub async fn remove_version(&self, id: &str, version: &str) -> Result<bool> {
        let path = self
            .root
            .join(crate::manifest::safe_segment(id)?)
            .join(crate::manifest::safe_segment(version)?);
        if !path.is_dir() {
            return Ok(false);
        }
        self.remove_installed(&path).await?;
        Ok(true)
    }

    /// The content digest of one installed version, in the form the revocation list names it.
    ///
    /// Hashed from the members on disk rather than read from anywhere: the digest *is* those
    /// bytes, which is what makes "withdraw the version I am looking at" name the package that
    /// is actually installed. Deliberately without a signature check — a package whose
    /// signature no longer covers what is on disk is precisely one a user may want to withdraw,
    /// and refusing to name it would leave the only unrevocable package the worst one.
    ///
    /// `None` when that version is not installed. The two segments are validated before they
    /// touch the path, because they arrive from a request and `..` would otherwise reach out of
    /// the plugin root.
    pub async fn installed_package_digest(
        &self,
        id: &str,
        version: &str,
    ) -> Result<Option<[u8; 32]>> {
        let path = self
            .root
            .join(crate::manifest::safe_segment(id)?)
            .join(crate::manifest::safe_segment(version)?);
        if !path.is_dir() {
            return Ok(None);
        }
        tokio::task::spawn_blocking(move || {
            let (manifest, component, _signature, locales) = read_package_parts(&path)?;
            Ok(Some(crate::package_digest(&manifest, &component, &locales)))
        })
        .await?
    }

    /// Rebuilds the provider registry's dynamic layer from what is installed right now.
    ///
    /// Since RD-101-13 a provider exists exactly as long as its plugin does, so every change to
    /// the installed set has to reach the registry. Installing already registered its row on
    /// the spot; removing and disabling only wrote to disk and left the row standing until the
    /// next start, which used to be invisible because a built-in row covered for it. Now it
    /// would mean an account could be created for a provider whose plugin had just been taken
    /// away.
    ///
    /// Rebuilt from the whole list rather than by unregistering one id: removing the newer of
    /// two installed versions has to hand the slug back to the older manifest, and that is not
    /// something a single removal can work out on its own.
    pub async fn refresh_providers(&self) {
        let packages = match self.verified_contents().await {
            Ok(packages) => packages,
            Err(error) => {
                tracing::warn!(error = %error, "could not refresh the provider registry");
                return;
            }
        };
        let rows: Vec<_> = packages
            .iter()
            .map(|package| &package.manifest)
            .filter(|manifest| !self.is_disabled(&manifest.id.to_string()))
            .filter_map(crate::provider_spec_from_manifest)
            .collect();
        for rejected in rd_provider_registry::replace_dynamic(rows) {
            tracing::warn!(error = %rejected, "ignoring plugin provider row");
        }
        // Rebuilt from the same list and in the same step (RD-110-38): a host whose fragment
        // is key material stops being one the moment its plugin is removed, and an intake
        // that still vaulted fragments for it would keep writing secrets nothing can read
        // back.
        rd_provider_registry::replace_secret_fragment_hosts(
            packages
                .iter()
                .map(|package| &package.manifest)
                .filter(|manifest| !self.is_disabled(&manifest.id.to_string()))
                .flat_map(|manifest| manifest.secret_fragment_domains.iter().cloned())
                .collect(),
        );
    }

    /// Re-verifies every installed package before it is compiled for execution.
    ///
    /// A package that no longer verifies — a revoked key, a tampered file — is skipped
    /// with a warning rather than aborting startup, so revoking trust disables one plugin
    /// instead of preventing the application from booting.
    pub async fn load_verified(&self) -> Result<Vec<VerifiedPackage>> {
        Ok(self
            .load_verified_with_roles()
            .await?
            .into_iter()
            .map(|(package, _)| package)
            .collect())
    }

    /// The same packages, each with its role under this start's version choice (RD-140-02).
    ///
    /// By id, then default, retained and staged, each group newest first — so a consumer that
    /// keeps the first entry per id keeps the default version, and one that wants a pinned
    /// version still finds it further down.
    pub async fn load_verified_with_roles(&self) -> Result<Vec<(VerifiedPackage, VersionRole)>> {
        let root = self.root.clone();
        let verifier = self.verifier.clone();
        let disabled = self
            .disabled
            .read()
            .map(|set| set.clone())
            .unwrap_or_default();
        let choices = self.version_choices();
        tokio::task::spawn_blocking(move || {
            load_verified_sync(&root, &verifier, &disabled, &choices)
        })
        .await?
    }

    /// The hex SHA-256 of every installed component that verifies, switched-off plugins and
    /// every kept version included: what the compile cache keeps entries for (RD-1240-34).
    ///
    /// # Errors
    ///
    /// When the plugin folder cannot be read.
    pub async fn installed_component_digests(&self) -> Result<std::collections::BTreeSet<String>> {
        let root = self.root.clone();
        let verifier = self.verifier.clone();
        let choices = self.version_choices();
        tokio::task::spawn_blocking(move || {
            let packages = load_verified_sync(
                &root,
                &verifier,
                &std::collections::HashSet::new(),
                &choices,
            )?;
            Ok(packages
                .iter()
                .map(|(package, _)| crate::compile_cache::hex_digest(&package.component))
                .collect::<std::collections::BTreeSet<_>>())
        })
        .await?
    }

    /// Verifies one installed version in full, as the next start would load it (RD-140-02).
    ///
    /// The health check before a version is made active or put under test: signature, content
    /// withdrawal, manifest, locales and the component's validation all have to pass, so a
    /// choice can never point at a package the next start would refuse. `None` when that
    /// version is not installed.
    pub async fn verify_installed_version(
        &self,
        id: &str,
        version: &str,
    ) -> Result<Option<PluginManifest>> {
        let path = self
            .root
            .join(crate::manifest::safe_segment(id)?)
            .join(crate::manifest::safe_segment(version)?);
        if !path.is_dir() {
            return Ok(None);
        }
        let verifier = self.verifier.clone();
        tokio::task::spawn_blocking(move || {
            load_one(&path, &verifier).map(|package| Some(package.manifest))
        })
        .await?
    }

    /// One installed version, verified and read in full exactly as a start loads it: the package
    /// a first install brings into the running service (RD-170-12). `None` when that version is
    /// not installed or its plugin is switched off, since a switched-off plugin never runs.
    pub async fn load_installed_version(
        &self,
        id: &str,
        version: &str,
    ) -> Result<Option<VerifiedPackage>> {
        if self.is_disabled(id) {
            return Ok(None);
        }
        let path = self
            .root
            .join(crate::manifest::safe_segment(id)?)
            .join(crate::manifest::safe_segment(version)?);
        if !path.is_dir() {
            return Ok(None);
        }
        let verifier = self.verifier.clone();
        tokio::task::spawn_blocking(move || load_one(&path, &verifier).map(Some)).await?
    }

    /// The manifest of every enabled installed package whose signature still covers what is on
    /// disk, without validating or compiling a single component (RD-120-51).
    ///
    /// For a question a manifest answers on its own -- which providers a `remote-job` plugin
    /// claims -- `load_verified` is the wrong price: it validates and compiles every installed
    /// component, of every type, and the remote-job page paid that on its first open after a
    /// start. The trust decision is the same here: signature, content revocation, manifest
    /// and locale validation, the app version, and the user's switch. What is left out is only
    /// what running a component needs, so a package whose component would fail to compile is
    /// still listed; its refusal comes when something actually runs it.
    pub async fn verified_manifests(&self) -> Result<Vec<PluginManifest>> {
        Ok(self
            .verified_contents()
            .await?
            .into_iter()
            .map(|package| package.manifest)
            .filter(|manifest| !self.is_disabled(&manifest.id.to_string()))
            .collect())
    }

    /// Installed packages whose signature still covers what is on disk, without compiling them.
    ///
    /// The provider rows and the locale bundle are built from this rather than from
    /// `list_installed`: both hand a manifest's own claims to the running system — request
    /// domains, cookie scope, secret slots, the strings the interface shows — and a manifest
    /// edited on disk used to keep contributing all of it while the same package was refused
    /// the moment its component was loaded. Disabled plugins are still included; switching a
    /// plugin off is a user decision that each caller applies for itself.
    async fn verified_contents(&self) -> Result<Vec<VerifiedContents>> {
        let root = self.root.clone();
        let verifier = self.verifier.clone();
        let choices = self.version_choices();
        tokio::task::spawn_blocking(move || verified_contents_sync(&root, &verifier, &choices))
            .await?
    }

    /// Merged vue-i18n message tree for `language`, across the default version of each plugin.
    ///
    /// Falls back to the manifest's own name and description when a plugin ships no
    /// translation for the requested language.
    pub async fn locale_bundle(&self, language: String) -> Result<serde_json::Value> {
        let root = self.root.clone();
        let verifier = self.verifier.clone();
        let choices = self.version_choices();
        tokio::task::spawn_blocking(move || {
            locale_bundle_sync(&root, &verifier, &choices, &language)
        })
        .await?
    }
}

/// An installed package reduced to what is read without running it, after its signature has
/// been checked against the bytes that are on disk now.
struct VerifiedContents {
    manifest: PluginManifest,
    /// `(language, raw JSON)` exactly as signed, so no caller has to re-read the file and
    /// trust whatever it finds there.
    locales: Vec<(String, Vec<u8>)>,
    /// Where this version stands under the start's version choice.
    role: VersionRole,
}

#[cfg(test)]
#[path = "installed_tests.rs"]
mod tests;
