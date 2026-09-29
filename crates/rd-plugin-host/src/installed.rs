//! Discovery and re-verification of versioned installed plugin packages.

use std::{collections::BTreeMap, io::Read, path::Path};

use anyhow::{Result, bail};

use crate::{
    MAX_COMPONENT_BYTES, MAX_LOCALE_BYTES, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES, ManifestHeader,
    ManifestRejection, PluginInstaller, PluginManifest, PluginVerifier, VerifiedPackage,
    VersionChoices, VersionRole, locales::parse_locale, manifest::validate_manifest,
    valid_language, versions::roles_by_id,
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

/// The default version of each plugin id, keyed by id.
///
/// `packages` comes from `verified_contents_sync`, which orders the default version of an id
/// before its others, so the first entry per id is the one to keep. A plugin whose only loaded
/// version is under test has no default and contributes nothing.
fn default_versions(packages: Vec<VerifiedContents>) -> BTreeMap<String, VerifiedContents> {
    let mut defaults: BTreeMap<String, VerifiedContents> = BTreeMap::new();
    for package in packages {
        if package.role != VersionRole::Default {
            continue;
        }
        defaults
            .entry(package.manifest.id.to_string())
            .or_insert(package);
    }
    defaults
}

fn locale_bundle_sync(
    root: &Path,
    verifier: &PluginVerifier,
    choices: &VersionChoices,
    language: &str,
) -> Result<serde_json::Value> {
    if !valid_language(language) {
        bail!("invalid locale language tag");
    }
    let mut codes = serde_json::Map::new();
    let mut providers = serde_json::Map::new();
    for package in default_versions(verified_contents_sync(root, verifier, choices)?).into_values()
    {
        let manifest = &package.manifest;
        let slug = manifest.message_slug().to_owned();
        let mut entry = serde_json::Map::new();
        entry.insert("name".to_owned(), manifest.name.clone().into());
        entry.insert(
            "description".to_owned(),
            manifest.metadata.description.clone().into(),
        );

        // The locale file as the signature covered it. Reading it off disk again would put
        // whatever is in that directory now into the interface of a package that is otherwise
        // refused for exactly that edit.
        if let Some((_, bytes)) = package
            .locales
            .iter()
            .find(|(tag, _)| tag.as_str() == language)
        {
            match parse_locale(&slug, language, bytes) {
                Ok(locale) => {
                    if let Some(name) = locale.name {
                        entry.insert("name".to_owned(), name.into());
                    }
                    if let Some(description) = locale.description {
                        entry.insert("description".to_owned(), description.into());
                    }
                    if let Some(account) = locale.account {
                        for (key, value) in [
                            ("secret_label", account.secret_label),
                            ("secret_hint", account.secret_hint),
                            ("secret_label_login", account.secret_label_login),
                            ("secret_hint_login", account.secret_hint_login),
                            ("secret_label_api_key", account.secret_label_api_key),
                            ("secret_hint_api_key", account.secret_hint_api_key),
                            ("secret_label_oauth", account.secret_label_oauth),
                            ("secret_hint_oauth", account.secret_hint_oauth),
                            ("mode_label_login", account.mode_label_login),
                            ("mode_label_api_key", account.mode_label_api_key),
                            ("mode_label_oauth", account.mode_label_oauth),
                            ("username_label", account.username_label),
                            ("username_hint", account.username_hint),
                        ] {
                            if let Some(value) = value {
                                entry.insert(key.to_owned(), value.into());
                            }
                        }
                    }
                    for (code, text) in locale.codes {
                        codes.insert(code, text.into());
                    }
                }
                Err(error) => tracing::warn!(
                    plugin = %manifest.name,
                    %language,
                    error = %error,
                    "skipping invalid plugin locale file"
                ),
            }
        }
        providers.insert(slug, entry.into());
    }
    Ok(serde_json::json!({
        "server": { "codes": codes },
        "providers": providers,
    }))
}

fn list_installed_sync(root: &Path) -> Result<Vec<PluginManifest>> {
    let mut manifests = Vec::new();
    for version in version_directories(root)? {
        let path = version.join("manifest.toml");
        let Some(bytes) = read_optional_bounded(&path, MAX_MANIFEST_BYTES)? else {
            continue;
        };
        let manifest: PluginManifest = match toml::from_slice(&bytes) {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::warn!(path = %path.display(), error = %error, "skipping unreadable plugin manifest");
                continue;
            }
        };
        if let Err(error) = validate_manifest(&manifest) {
            tracing::warn!(path = %path.display(), error = %error, "skipping invalid plugin manifest");
            continue;
        }
        manifests.push(manifest);
    }
    manifests.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| version_cmp_desc(&left.version, &right.version))
    });
    Ok(manifests)
}

fn list_incompatible_sync(root: &Path) -> Result<Vec<IncompatiblePlugin>> {
    let mut refused = Vec::new();
    for version in version_directories(root)? {
        let path = version.join("manifest.toml");
        let Some(bytes) = read_optional_bounded(&path, MAX_MANIFEST_BYTES)? else {
            continue;
        };
        let code = match toml::from_slice::<PluginManifest>(&bytes) {
            Ok(manifest) => match validate_manifest(&manifest) {
                Ok(()) => continue,
                Err(error) => error
                    .downcast_ref::<ManifestRejection>()
                    .map_or("plugin.manifest_unreadable", ManifestRejection::code),
            },
            // A manifest of another revision does not even deserialise into this one, so the
            // shared header is the only thing that can still tell outdated from broken.
            Err(_) => "plugin.manifest_outdated",
        };
        let header = toml::from_slice::<ManifestHeader>(&bytes).ok();
        let code = match &header {
            Some(header) if header.manifest_version == crate::MANIFEST_VERSION => code,
            Some(_) => "plugin.manifest_outdated",
            // Too broken to name itself — and exactly the package a user needs help finding.
            None => "plugin.manifest_unreadable",
        };
        refused.push(IncompatiblePlugin {
            id: header.as_ref().map_or_else(
                || directory_name(version.parent()),
                |header| header.id.to_string(),
            ),
            name: header.as_ref().map_or_else(
                || directory_name(version.parent()),
                |header| header.name.clone(),
            ),
            version: header.as_ref().map_or_else(
                || directory_name(Some(version.as_path())),
                |header| header.version.clone(),
            ),
            code: code.to_owned(),
        });
    }
    refused.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| version_cmp_desc(&left.version, &right.version))
    });
    Ok(refused)
}

/// The directory's own name, used when a manifest cannot say what it is.
fn directory_name(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or("unknown")
        .to_owned()
}

fn load_verified_sync(
    root: &Path,
    verifier: &PluginVerifier,
    disabled: &std::collections::HashSet<String>,
    choices: &VersionChoices,
) -> Result<Vec<(VerifiedPackage, VersionRole)>> {
    let _batch = crate::unsigned_notice::UnsignedBatch::open();
    let mut packages = Vec::new();
    for version in version_directories(root)? {
        match load_one(&version, verifier) {
            Ok(package) if disabled.contains(&package.manifest.id.to_string()) => {
                tracing::debug!(plugin = %package.manifest.name, "skipping plugin switched off by the user");
            }
            Ok(package) => packages.push(package),
            Err(error) => tracing::warn!(
                path = %version.display(),
                error = %error,
                "skipping installed plugin that no longer verifies"
            ),
        }
    }
    // Roles are decided over what actually loaded: a withdrawn or tampered version never
    // got this far, so it can be neither the default nor the one under test.
    let roles = roles_by_id(
        &packages
            .iter()
            .map(|package| {
                (
                    package.manifest.id.to_string(),
                    package.manifest.version.clone(),
                )
            })
            .collect::<Vec<_>>(),
        choices,
    );
    let mut packages: Vec<(VerifiedPackage, VersionRole)> = packages
        .into_iter()
        .map(|package| {
            let role = roles
                .get(&(
                    package.manifest.id.to_string(),
                    package.manifest.version.clone(),
                ))
                .copied()
                .unwrap_or(VersionRole::Retained);
            (package, role)
        })
        .collect();
    // By id, then role, then newest version first — the order this result is actually
    // consumed in. Every adapter in `rd-plugin-ext` and the transfer registry deduplicate by
    // `id` and keep the first entry they see, so ordering by display name left a plugin
    // *renamed* between versions with its two versions apart, and "first wins" then picked
    // whichever name sorted first instead of the default version. Nothing displays this list:
    // the Plugins view is built from `list_installed`, which keeps its by-name display order.
    packages.sort_by(|(left, left_role), (right, right_role)| {
        left.manifest
            .id
            .cmp(&right.manifest.id)
            .then_with(|| left_role.cmp(right_role))
            .then_with(|| version_cmp_desc(&left.manifest.version, &right.manifest.version))
    });
    Ok(packages)
}

/// Every installed package whose signature still covers the bytes on disk, in display order.
///
/// The component is read although nothing here will run it: the digest is taken over the
/// manifest, the component and the locale files together, so the signature cannot be checked
/// without it. It is dropped again with the rest of the package, because keeping one per
/// installed plugin would hold up to `MAX_COMPONENT_BYTES` each just to list manifests.
fn verified_contents_sync(
    root: &Path,
    verifier: &PluginVerifier,
    choices: &VersionChoices,
) -> Result<Vec<VerifiedContents>> {
    let _batch = crate::unsigned_notice::UnsignedBatch::open();
    let mut packages = Vec::new();
    for version in version_directories(root)? {
        match load_one_contents(&version, verifier) {
            Ok(package) => packages.push(package),
            Err(error) => tracing::warn!(
                path = %version.display(),
                error = %error,
                "skipping installed plugin whose manifest no longer verifies"
            ),
        }
    }
    let roles = roles_by_id(
        &packages
            .iter()
            .map(|package| {
                (
                    package.manifest.id.to_string(),
                    package.manifest.version.clone(),
                )
            })
            .collect::<Vec<_>>(),
        choices,
    );
    for package in &mut packages {
        if let Some(role) = roles.get(&(
            package.manifest.id.to_string(),
            package.manifest.version.clone(),
        )) {
            package.role = *role;
        }
    }
    // The same display order `list_installed` uses, with the default version of an id ahead of
    // its other versions of the same name: the provider registry keeps the first row per id,
    // so this is what makes the provider row the one of the version that runs.
    packages.sort_by(|left, right| {
        left.manifest
            .name
            .to_lowercase()
            .cmp(&right.manifest.name.to_lowercase())
            .then_with(|| left.role.cmp(&right.role))
            .then_with(|| version_cmp_desc(&left.manifest.version, &right.manifest.version))
    });
    Ok(packages)
}

fn load_one(version: &Path, verifier: &PluginVerifier) -> Result<VerifiedPackage> {
    let (manifest, component, signature, locales) = read_package_parts(version)?;
    verifier
        .verify_parts(manifest, component, signature, locales)
        .map_err(Into::into)
}

fn load_one_contents(version: &Path, verifier: &PluginVerifier) -> Result<VerifiedContents> {
    let (manifest, component, signature, locales) = read_package_parts(version)?;
    let package = verifier.verify_contents(manifest, component, signature, locales)?;
    Ok(VerifiedContents {
        manifest: package.manifest,
        locales: package.locales,
        // Decided once the whole list is known; see `verified_contents_sync`.
        role: VersionRole::Retained,
    })
}

type PackageParts = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<(String, Vec<u8>)>);

fn read_package_parts(version: &Path) -> Result<PackageParts> {
    Ok((
        read_bounded_file(&version.join("manifest.toml"), MAX_MANIFEST_BYTES)?,
        read_bounded_file(&version.join("component.wasm"), MAX_COMPONENT_BYTES)?,
        read_optional_bounded(&version.join("signature.ed25519"), MAX_SIGNATURE_BYTES)?,
        read_locales(&version.join("locales"))?,
    ))
}

fn read_locales(directory: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut locales = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(language) = name.strip_suffix(".json").filter(|tag| valid_language(tag)) else {
            bail!("unexpected file {name} in installed locales directory");
        };
        locales.push((
            language.to_owned(),
            read_bounded_file(&entry.path(), MAX_LOCALE_BYTES)?,
        ));
    }
    locales.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(locales)
}

/// Every `<root>/<plugin-id>/<version>` directory, ignoring stray files and the staging
/// directory of an install that stopped before its rename (RD-170-07): loading one would list a
/// second copy of the plugin that `remove_version` cannot address.
fn version_directories(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut directories = Vec::new();
    for plugin in std::fs::read_dir(root)? {
        let plugin = plugin?;
        if !plugin.file_type()?.is_dir() {
            continue;
        }
        for version in std::fs::read_dir(plugin.path())? {
            let version = version?;
            if version.file_type()?.is_dir() && !is_install_staging(&version.file_name()) {
                directories.push(version.path());
            }
        }
    }
    Ok(directories)
}

fn is_install_staging(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with(crate::INSTALL_STAGING_PREFIX))
}

/// Every staging directory an install left under `root` without renaming it into place.
fn install_stagings(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut stagings = Vec::new();
    if !root.exists() {
        return Ok(stagings);
    }
    for plugin in std::fs::read_dir(root)? {
        let plugin = plugin?;
        if !plugin.file_type()?.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(plugin.path())? {
            let entry = entry?;
            if entry.file_type()?.is_dir() && is_install_staging(&entry.file_name()) {
                stagings.push(entry.path());
            }
        }
    }
    Ok(stagings)
}

fn version_cmp_desc(left: &str, right: &str) -> std::cmp::Ordering {
    semver::Version::parse(right)
        .ok()
        .cmp(&semver::Version::parse(left).ok())
}

fn read_optional_bounded(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    match read_bounded_file(path, limit) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn read_bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > limit {
        bail!("plugin file {} exceeds its size limit", path.display());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("plugin file {} exceeds its size limit", path.display());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use base64::{Engine, engine::general_purpose::STANDARD};
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;
    use crate::{package_digest, public_key_base64, tests::fixture_manifest};

    const EMPTY_COMPONENT: &[u8] = b"\0asm\x0d\0\x01\0";

    fn signed_archive(signing: &SigningKey, locales: &[(String, Vec<u8>)]) -> (Vec<u8>, Vec<u8>) {
        signed_archive_named(signing, "Fixture", "1.2.3", locales)
    }

    /// The same fixture under a chosen name and version, so one plugin id can be installed
    /// twice — including under two different names, which is the case the ordering must hold
    /// for.
    fn signed_archive_named(
        signing: &SigningKey,
        name: &str,
        version: &str,
        locales: &[(String, Vec<u8>)],
    ) -> (Vec<u8>, Vec<u8>) {
        let manifest = fixture_manifest(&public_key_base64(signing))
            .replace("name = \"Fixture\"", &format!("name = \"{name}\""))
            .replace("version = \"1.2.3\"", &format!("version = \"{version}\""))
            .into_bytes();
        let signature = STANDARD.encode(
            signing
                .sign(&package_digest(&manifest, EMPTY_COMPONENT, locales))
                .to_bytes(),
        );
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (member, content) in [
            ("manifest.toml".to_owned(), manifest.clone()),
            ("component.wasm".to_owned(), EMPTY_COMPONENT.to_vec()),
            ("signature.ed25519".to_owned(), signature.into_bytes()),
        ] {
            writer.start_file(&member, options).expect("archive member");
            writer.write_all(&content).expect("archive content");
        }
        for (language, bytes) in locales {
            writer
                .start_file(format!("locales/{language}.json"), options)
                .expect("locale member");
            writer.write_all(bytes).expect("locale content");
        }
        (writer.finish().expect("archive").into_inner(), manifest)
    }

    fn installer_with(signing: &SigningKey, root: &Path) -> PluginInstaller {
        let verifier = PluginVerifier::new(false);
        verifier
            .trust_key("fixture-v1".to_owned(), signing.verifying_key())
            .expect("trust key");
        PluginInstaller::new(root.to_owned(), verifier)
    }

    #[tokio::test]
    async fn installed_signature_is_retained_and_reverified() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[9_u8; 32]);
        let (archive, _) = signed_archive(&signing, &[]);
        let installer = installer_with(&signing, directory.path());

        let installed = installer.install_bytes(archive).await.expect("install");
        assert!(installed.path.join("signature.ed25519").is_file());
        let loaded = installer.load_verified().await.expect("reload");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].manifest.message_slug(), "fixture");
    }

    #[tokio::test]
    async fn locale_files_survive_install_and_reverification() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[11_u8; 32]);
        let locales = vec![
            (
                "en".to_owned(),
                br#"{"name":"Fixture","codes":{"fixture.oops":"Oops"}}"#.to_vec(),
            ),
            (
                "de".to_owned(),
                br#"{"name":"Fixture DE","codes":{"fixture.oops":"Hoppla"}}"#.to_vec(),
            ),
        ];
        let (archive, _) = signed_archive(&signing, &locales);
        let installer = installer_with(&signing, directory.path());

        let installed = installer.install_bytes(archive).await.expect("install");
        assert!(installed.path.join("locales").join("de.json").is_file());
        let loaded = installer.load_verified().await.expect("reload");
        assert_eq!(
            loaded.len(),
            1,
            "locale files must not break re-verification"
        );

        let bundle = installer
            .locale_bundle("de".to_owned())
            .await
            .expect("bundle");
        assert_eq!(bundle["server"]["codes"]["fixture.oops"], "Hoppla");
        assert_eq!(bundle["providers"]["fixture"]["name"], "Fixture DE");
        // Untranslated languages fall back to the manifest's own strings.
        let english = installer
            .locale_bundle("fr".to_owned())
            .await
            .expect("bundle");
        assert_eq!(english["providers"]["fixture"]["name"], "Fixture");
        assert_eq!(
            english["providers"]["fixture"]["description"],
            "A fixture resolver"
        );
    }

    #[tokio::test]
    async fn revoking_a_key_skips_the_plugin_instead_of_failing_startup() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[13_u8; 32]);
        let (archive, _) = signed_archive(&signing, &[]);
        let installer = installer_with(&signing, directory.path());
        installer.install_bytes(archive).await.expect("install");
        assert_eq!(installer.load_verified().await.expect("reload").len(), 1);

        assert!(
            installer
                .verifier()
                .revoke_key("fixture-v1")
                .expect("revoke")
        );
        let loaded = installer
            .load_verified()
            .await
            .expect("startup must still succeed");
        assert!(loaded.is_empty(), "revoked plugin must be skipped");
    }

    /// Withdrawing one published version must take that version out and nothing else — that
    /// is the whole reason content revocation exists next to key revocation.
    #[tokio::test]
    async fn a_revoked_digest_drops_one_version_and_leaves_the_key_trusted() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[19_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        let (withdrawn, withdrawn_manifest) =
            signed_archive_named(&signing, "Fixture", "2.0.0", &[]);
        let (kept, _) = signed_archive_named(&signing, "Fixture", "1.0.0", &[]);
        installer.install_bytes(withdrawn).await.expect("install");
        installer.install_bytes(kept).await.expect("install");
        assert_eq!(installer.load_verified().await.expect("reload").len(), 2);

        let digest = package_digest(&withdrawn_manifest, EMPTY_COMPONENT, &[]);
        assert!(
            installer
                .verifier()
                .revoke_package_digest(digest)
                .expect("revoke the digest"),
            "the first withdrawal of a digest has to report that it is new"
        );
        assert!(
            !installer
                .verifier()
                .revoke_package_digest(digest)
                .expect("revoke the digest again"),
            "repeating a withdrawal must be distinguishable from making one"
        );
        assert!(
            installer
                .verifier()
                .is_package_revoked(&digest)
                .expect("read")
        );

        let loaded = installer
            .load_verified()
            .await
            .expect("startup must still succeed");
        assert_eq!(loaded.len(), 1, "the withdrawn version must be skipped");
        assert_eq!(loaded[0].manifest.version, "1.0.0");
        assert!(
            installer.verifier().is_trusted("fixture-v1").expect("read"),
            "withdrawing one package must not revoke its author's key"
        );
    }

    /// The withdrawn set is process state, so the only thing between a restart and a package
    /// that was withdrawn yesterday is the seeding. This covers that seam end to end: the hex
    /// digest a caller stored goes back in, the package stays out of the next load, and taking
    /// the withdrawal back lets it load again — at the next load, not in mid-session.
    #[tokio::test]
    async fn a_seeded_digest_keeps_its_package_out_until_the_withdrawal_is_taken_back() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[29_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        let (archive, manifest_bytes) = signed_archive(&signing, &[]);
        let installed = installer.install_bytes(archive).await.expect("install");

        // What the API would store: the digest of the version the user is looking at.
        let digest = installer
            .installed_package_digest(
                &installed.manifest.id.to_string(),
                &installed.manifest.version,
            )
            .await
            .expect("read the installed digest")
            .expect("the version is installed");
        assert_eq!(
            digest,
            package_digest(&manifest_bytes, EMPTY_COMPONENT, &[])
        );
        let stored = crate::format_package_digest(&digest);

        // A restart: a fresh verifier holds nothing, and only the seeding keeps it from
        // loading a package that was withdrawn before this process existed.
        let restarted = installer_with(&signing, directory.path());
        assert_eq!(restarted.load_verified().await.expect("reload").len(), 1);
        restarted
            .verifier()
            .set_revoked_package_digests([
                crate::parse_package_digest(&stored).expect("the stored form parses back")
            ])
            .expect("seed the withdrawn set");
        assert!(
            restarted.load_verified().await.expect("reload").is_empty(),
            "a seeded digest has to survive the restart it was seeded for"
        );
        assert_eq!(
            restarted
                .verifier()
                .revoked_package_digests()
                .expect("list"),
            vec![stored]
        );

        assert!(
            restarted
                .verifier()
                .unrevoke_package_digest(&digest)
                .expect("take the withdrawal back")
        );
        assert_eq!(restarted.load_verified().await.expect("reload").len(), 1);
    }

    /// An id or version from a request must not be able to leave the plugin root, and a
    /// version that is not installed has to be a plain "nothing here" rather than an error.
    #[tokio::test]
    async fn a_digest_is_only_read_for_a_version_inside_the_plugin_root() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[31_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        assert!(
            installer
                .installed_package_digest("019d0000-0000-7000-8000-00000000abcd", "9.9.9")
                .await
                .expect("a missing version is not an error")
                .is_none()
        );
        assert!(
            installer
                .installed_package_digest("..", "1.2.3")
                .await
                .is_err(),
            "a traversal segment must be refused rather than resolved"
        );
    }

    /// A manifest edited after installation has to stop counting everywhere, not only where a
    /// component is compiled. The provider rows and the locale bundle are what such an edit is
    /// worth tampering for — request domains, cookie scope, secret slots, the strings shown for
    /// an account — and they used to be read straight off disk with no signature check.
    #[tokio::test]
    async fn a_tampered_manifest_stops_reaching_the_locale_bundle() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[23_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        let (archive, _) = signed_archive(&signing, &[]);
        let installed = installer.install_bytes(archive).await.expect("install");
        let bundle = installer
            .locale_bundle("en".to_owned())
            .await
            .expect("bundle");
        assert_eq!(bundle["providers"]["fixture"]["name"], "Fixture");

        let manifest_path = installed.path.join("manifest.toml");
        let manifest = std::fs::read_to_string(&manifest_path).expect("read the manifest");
        std::fs::write(
            &manifest_path,
            manifest.replace("A fixture resolver", "Anything the attacker likes"),
        )
        .expect("tamper with the manifest");

        // The same package is already skipped by `load_verified`; this is the other path.
        assert!(installer.load_verified().await.expect("reload").is_empty());
        let bundle = installer
            .locale_bundle("en".to_owned())
            .await
            .expect("bundle");
        assert!(
            bundle["providers"].get("fixture").is_none(),
            "a manifest that no longer verifies must not reach the interface bundle"
        );
    }

    /// Consumers deduplicate by id and keep the first entry, so the versions of one plugin
    /// have to be adjacent and newest first even when the plugin was renamed in between.
    #[tokio::test]
    async fn a_renamed_plugin_still_yields_its_newest_version_first() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[21_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        // The newer version sorts last by name, which is exactly what used to hide it.
        for (name, version) in [("Zulu Storage", "2.0.0"), ("Alpha Storage", "1.0.0")] {
            let (archive, _) = signed_archive_named(&signing, name, version, &[]);
            installer.install_bytes(archive).await.expect("install");
        }

        let loaded = installer.load_verified().await.expect("reload");
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded[0].manifest.version, "2.0.0",
            "the newest version must come first whatever it is called"
        );
        assert_eq!(loaded[1].manifest.version, "1.0.0");
    }

    /// The version choice of RD-140-02 as the adapters see it: the default version first, a
    /// staged one never among the versions an adapter picks from, and a withdrawn version
    /// neither the default nor under test.
    #[tokio::test]
    async fn the_version_choice_decides_the_order_the_adapters_pick_from() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[23_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        let mut manifests = std::collections::HashMap::new();
        for version in ["1.0.0", "2.0.0", "3.0.0"] {
            let (archive, manifest) = signed_archive_named(&signing, "Fixture", version, &[]);
            installer.install_bytes(archive).await.expect("install");
            manifests.insert(version, manifest);
        }
        let id = installer.load_verified().await.expect("load")[0]
            .manifest
            .id
            .to_string();
        let order = |loaded: Vec<(VerifiedPackage, VersionRole)>| -> Vec<(String, VersionRole)> {
            loaded
                .into_iter()
                .map(|(package, role)| (package.manifest.version, role))
                .collect()
        };

        // Rolled back to 1.0.0, 3.0.0 under test.
        installer.set_version_choices(
            [(
                id.clone(),
                crate::VersionChoice {
                    active: Some("1.0.0".to_owned()),
                    staged: Some("3.0.0".to_owned()),
                },
            )]
            .into(),
        );
        assert_eq!(
            order(installer.load_verified_with_roles().await.expect("load")),
            vec![
                ("1.0.0".to_owned(), VersionRole::Default),
                ("2.0.0".to_owned(), VersionRole::Retained),
                ("3.0.0".to_owned(), VersionRole::Staged),
            ]
        );
        let registry = crate::PluginTypeRegistry::load(&installer)
            .await
            .expect("registry");
        let offered: Vec<&str> = registry
            .of_type(&crate::PluginType::Resolver)
            .map(|package| package.manifest.version.as_str())
            .collect();
        assert_eq!(
            offered,
            ["1.0.0", "2.0.0"],
            "the version under test is reached through a pin, never through `of_type`"
        );

        // Withdrawing the chosen version: it does not load, so the newest version that is not
        // under test takes over, and the choice needs no repair to stay safe.
        installer
            .verifier()
            .revoke_package_digest(package_digest(&manifests["1.0.0"], EMPTY_COMPONENT, &[]))
            .expect("withdraw");
        assert_eq!(
            order(installer.load_verified_with_roles().await.expect("load")),
            vec![
                ("2.0.0".to_owned(), VersionRole::Default),
                ("3.0.0".to_owned(), VersionRole::Staged),
            ]
        );

        // And withdrawing the staged one leaves nothing under test.
        installer
            .verifier()
            .revoke_package_digest(package_digest(&manifests["3.0.0"], EMPTY_COMPONENT, &[]))
            .expect("withdraw");
        assert_eq!(
            order(installer.load_verified_with_roles().await.expect("load")),
            vec![("2.0.0".to_owned(), VersionRole::Default)]
        );
    }

    /// The health check before a choice: a withdrawn version fails it like the next start would.
    #[tokio::test]
    async fn a_withdrawn_version_does_not_pass_the_check_before_activation() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[24_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        let (archive, manifest) = signed_archive_named(&signing, "Fixture", "1.0.0", &[]);
        installer.install_bytes(archive).await.expect("install");
        let id = installer.load_verified().await.expect("load")[0]
            .manifest
            .id
            .to_string();

        assert!(
            installer
                .verify_installed_version(&id, "1.0.0")
                .await
                .expect("check")
                .is_some()
        );
        assert!(
            installer
                .verify_installed_version(&id, "9.9.9")
                .await
                .expect("a missing version is not an error")
                .is_none()
        );
        installer
            .verifier()
            .revoke_package_digest(package_digest(&manifest, EMPTY_COMPONENT, &[]))
            .expect("withdraw");
        assert!(
            installer
                .verify_installed_version(&id, "1.0.0")
                .await
                .is_err()
        );
    }

    /// `plugin.before_version_promoted` (RD-170-07, recovery matrix): an update stopped after
    /// its package was written under the staging name and before the rename that makes it a
    /// version.
    #[cfg(feature = "failpoints")]
    #[tokio::test]
    async fn an_update_stopped_before_its_rename_is_never_loaded_and_the_start_removes_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let signing = SigningKey::from_bytes(&[23_u8; 32]);
        let installer = installer_with(&signing, directory.path());
        let (current, _) = signed_archive_named(&signing, "Fixture", "1.2.3", &[]);
        installer.install_bytes(current).await.expect("install");

        let (update, _) = signed_archive_named(&signing, "Fixture", "1.2.4", &[]);
        {
            let guard = rd_core::failpoint::FailpointGuard::once("plugin.before_version_promoted");
            assert!(installer.install_bytes(update.clone()).await.is_err());
            assert!(guard.fired(), "the crash point was never reached");
        }
        assert_eq!(install_stagings(directory.path()).expect("list").len(), 1);
        let versions = |manifests: Vec<PluginManifest>| -> Vec<String> {
            manifests
                .into_iter()
                .map(|manifest| manifest.version)
                .collect()
        };
        // The installed version stays the only one, before and after the restart.
        assert_eq!(
            versions(installer.list_installed().await.expect("list")),
            ["1.2.3"]
        );

        let restarted = installer_with(&signing, directory.path());
        assert_eq!(restarted.load_verified().await.expect("load").len(), 1);
        assert_eq!(restarted.sweep_install_staging().await, 1);
        assert!(install_stagings(directory.path()).expect("list").is_empty());
        // The next update pass installs it as if nothing had happened.
        restarted.install_bytes(update).await.expect("update");
        let mut installed = versions(restarted.list_installed().await.expect("list"));
        installed.sort();
        assert_eq!(installed, ["1.2.3", "1.2.4"]);
    }
}
