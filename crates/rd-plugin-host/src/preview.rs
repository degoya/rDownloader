//! What a package is before anything installs it (RD-140-01): name, version, publisher,
//! permissions and whether its key is trusted.
//!
//! Installing used to be the first thing that looked at a package, so a package signed by an
//! already trusted key went onto disk without anybody having seen what it asks for. This reads
//! the same archive the installer reads, proves the package is signed by the key its own
//! manifest names — so the publisher shown is the one that signed it — and reports the trust
//! decision instead of making it. Nothing is written and no component is compiled: a preview of
//! a package nobody installs must cost nothing but the read.

use std::io::Cursor;

use anyhow::{Context, Result, ensure};
use rd_core::PluginId;
use serde::Serialize;

use crate::{
    APP_VERSION, PluginManifest, PluginType, PluginVerifier, check_app_version,
    format_package_digest,
    index::{MAX_PACKAGE_BYTES, Permissions, Publisher},
    key_fingerprint,
    manifest::validate_manifest,
    package_digest, read_archive, validate_locales,
};

/// Where the signing key stands on this installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum KeyStatus {
    /// Installing needs no further decision.
    Trusted,
    /// Installing asks for the key to be confirmed first, by this fingerprint.
    Untrusted,
    /// The key id is trusted, but for a different key: installing is refused.
    Mismatch,
    /// A repository withdrew this key: installing is refused, prompt included.
    Withdrawn,
    /// No signature; only a development-mode installation takes it.
    Unsigned,
}

/// Everything the install preview shows.
#[derive(Clone, Debug, Serialize)]
pub struct PackagePreview {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub plugin_type: PluginType,
    pub api_version: String,
    pub min_app_version: Option<String>,
    pub description: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    /// `package_digest` in 64 lowercase hex characters.
    pub package_digest: String,
    pub size: u64,
    /// Who signed it; `None` for an unsigned package.
    pub publisher: Option<Publisher>,
    pub permissions: Permissions,
    pub key: KeyStatus,
    /// Whether this exact package was withdrawn by its digest.
    pub withdrawn: bool,
    /// Why this build cannot run it, when it cannot. Informational: the install refuses it
    /// with the same reason, so the preview says so before anybody confirms anything.
    pub incompatible: Option<String>,
}

impl PackagePreview {
    /// Whether confirming this preview can install anything at all.
    #[must_use]
    pub fn installable(&self) -> bool {
        !self.withdrawn
            && self.incompatible.is_none()
            && !matches!(self.key, KeyStatus::Mismatch | KeyStatus::Withdrawn)
    }
}

/// Reads one `.rdplug` and describes it without installing it.
///
/// Refuses what is not a package at all — an unreadable archive, a manifest that does not
/// parse, a signature that does not hold for the key the manifest names. Everything else is a
/// finding, not a refusal: a package for another contract is described and marked
/// incompatible, so the person sees *why* before the install refuses it.
pub fn preview_package(bytes: &[u8], verifier: &PluginVerifier) -> Result<PackagePreview> {
    let size = u64::try_from(bytes.len())?;
    ensure!(
        size > 0 && size <= MAX_PACKAGE_BYTES,
        "the package is empty or larger than {MAX_PACKAGE_BYTES} bytes"
    );
    let (manifest_bytes, component, signature, locales) = read_archive(Cursor::new(bytes))?;
    let manifest: PluginManifest =
        toml::from_slice(&manifest_bytes).context("parse plugin manifest")?;
    let digest = package_digest(&manifest_bytes, &component, &locales);
    let incompatible = validate_manifest(&manifest)
        .and_then(|()| check_app_version(&manifest, APP_VERSION))
        .and_then(|()| validate_locales(manifest.message_slug(), &locales))
        .err()
        .map(|error| format!("{error:#}"));
    let (publisher, key) = match signature {
        Some(signature) => {
            let declared = manifest.verifying_key()?;
            rd_sign::verify_detached_bytes(&declared, &digest, &signature)
                .context("the package is not signed by the key its manifest names")?;
            let fingerprint = key_fingerprint(&declared);
            let key = if verifier.withdrawn_keys.contains(&fingerprint)? {
                KeyStatus::Withdrawn
            } else {
                match verifier.trusted_keys.key(&manifest.key_id)? {
                    Some(trusted) if trusted == declared => KeyStatus::Trusted,
                    Some(_) => KeyStatus::Mismatch,
                    None => KeyStatus::Untrusted,
                }
            };
            let publisher = Publisher {
                key_id: manifest.key_id.clone(),
                fingerprint,
                author: manifest.metadata.author.clone(),
            };
            (Some(publisher), key)
        }
        None => (None, KeyStatus::Unsigned),
    };
    let withdrawn = verifier.revoked.contains(&digest)?;
    Ok(PackagePreview {
        id: manifest.id,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        plugin_type: manifest.plugin_type.clone(),
        api_version: manifest.api_version.clone(),
        min_app_version: manifest.metadata.min_app_version.clone(),
        description: manifest.metadata.description.clone(),
        homepage: manifest.metadata.homepage.clone(),
        license: manifest.metadata.license.clone(),
        package_digest: format_package_digest(&digest),
        size,
        publisher,
        permissions: Permissions::of(&manifest),
        key,
        withdrawn,
        incompatible,
    })
}

/// The content digest of a `.rdplug`, as the index names packages.
///
/// Reads the archive the way the installer does, so a package whose zip framing differs but
/// whose members are the same bytes still matches — and one with a single changed byte in any
/// member does not.
pub fn archive_digest(bytes: &[u8]) -> Result<[u8; 32]> {
    let (manifest_bytes, component, _signature, locales) = read_archive(Cursor::new(bytes))?;
    Ok(package_digest(&manifest_bytes, &component, &locales))
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
