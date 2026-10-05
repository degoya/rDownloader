//! The blocking half of the installed-package listing: walking the version directories,
//! reading each package's members with a size bound, re-verifying them and ordering the
//! result. Every function here runs inside `spawn_blocking` on behalf of a `PluginInstaller`
//! method in the parent module.
//!
//! Split out of `installed.rs` (PLUG-21).

use std::{collections::BTreeMap, io::Read, path::Path};

use anyhow::{Result, bail};

use super::{IncompatiblePlugin, VerifiedContents};
use crate::{
    MAX_COMPONENT_BYTES, MAX_LOCALE_BYTES, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES, ManifestHeader,
    ManifestRejection, PluginManifest, PluginVerifier, VerifiedPackage, VersionChoices,
    VersionRole, locales::parse_locale, manifest::validate_manifest, valid_language,
    versions::roles_by_id,
};

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

pub(super) fn locale_bundle_sync(
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

pub(super) fn list_installed_sync(root: &Path) -> Result<Vec<PluginManifest>> {
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

pub(super) fn list_incompatible_sync(root: &Path) -> Result<Vec<IncompatiblePlugin>> {
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

pub(super) fn load_verified_sync(
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
pub(super) fn verified_contents_sync(
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

pub(super) fn load_one(version: &Path, verifier: &PluginVerifier) -> Result<VerifiedPackage> {
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

pub(super) fn read_package_parts(version: &Path) -> Result<PackageParts> {
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
pub(super) fn install_stagings(root: &Path) -> Result<Vec<std::path::PathBuf>> {
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
