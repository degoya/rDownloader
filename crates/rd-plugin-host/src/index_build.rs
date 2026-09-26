//! The publishing half of [`super`]: describing a signed `.rdplug` as an index entry.

use std::io::Cursor;

use anyhow::{Context, Result, bail, ensure};

use super::{IndexPackage, MAX_PACKAGE_BYTES, Permissions, Publisher};
use crate::{
    PluginManifest, format_package_digest, key_fingerprint, manifest::validate_manifest,
    package_digest, read_archive, validate_locales,
};

/// Reads one signed package and describes it as an index entry.
///
/// Proves the package is signed by the key its own manifest names — the same internal
/// consistency the verifier checks before it asks about an unknown key — but not that the key
/// is trusted: that stays the installing side's decision, and a repository cannot make it. An
/// unsigned package is refused, because no installation outside development mode would take
/// it. The component is read and hashed, never compiled: the packager already did that, and
/// the index describes bytes rather than running them.
pub fn describe_package(
    bytes: &[u8],
    url: String,
    release_notes: Option<String>,
) -> Result<IndexPackage> {
    let size = u64::try_from(bytes.len())?;
    ensure!(
        size <= MAX_PACKAGE_BYTES,
        "the package is larger than {MAX_PACKAGE_BYTES} bytes"
    );
    let (manifest_bytes, component, signature, locales) = read_archive(Cursor::new(bytes))?;
    let manifest: PluginManifest =
        toml::from_slice(&manifest_bytes).context("parse plugin manifest")?;
    validate_manifest(&manifest)?;
    validate_locales(manifest.message_slug(), &locales)?;
    let label = format!("{} {}", manifest.name, manifest.version);
    let Some(signature) = signature else {
        bail!("{label} is unsigned; an index lists signed packages only");
    };
    let key = manifest.verifying_key()?;
    let digest = package_digest(&manifest_bytes, &component, &locales);
    rd_sign::verify_detached_bytes(&key, &digest, &signature)
        .with_context(|| format!("{label} is not signed by the key its manifest names"))?;
    let release_notes = release_notes
        .map(|notes| notes.replace("\r\n", "\n").trim().to_owned())
        .filter(|notes| !notes.is_empty());
    Ok(IndexPackage {
        id: manifest.id,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        plugin_type: manifest.plugin_type.clone(),
        api_version: manifest.api_version.clone(),
        min_app_version: manifest.metadata.min_app_version.clone(),
        package_digest: format_package_digest(&digest),
        size,
        url,
        publisher: Publisher {
            key_id: manifest.key_id.clone(),
            fingerprint: key_fingerprint(&key),
            author: manifest.metadata.author.clone(),
        },
        permissions: Permissions::of(&manifest),
        release_notes,
    })
}

/// The `url` an entry for `file_name` carries.
///
/// Without a base it is the bare file name, resolved against the index's own URL — right for a
/// repository whose index sits beside its packages. With one it is joined onto `base`, which
/// has to be `https://` and end in `/`: without the slash, `Url::join` replaces the last path
/// segment instead of descending into it, and every entry would point one directory too high.
pub fn package_url(base: Option<&str>, file_name: &str) -> Result<String> {
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains(['/', '\\', ':', '?', '#'])
    {
        bail!("{file_name:?} is not a plain file name");
    }
    let Some(base) = base else {
        return Ok(file_name.to_owned());
    };
    let parsed = url::Url::parse(base).with_context(|| format!("parse base URL {base}"))?;
    ensure!(
        parsed.scheme() == "https",
        "the base URL {base} is not https"
    );
    ensure!(
        parsed.path().ends_with('/'),
        "the base URL {base} must end with '/'"
    );
    ensure!(
        parsed.query().is_none() && parsed.fragment().is_none(),
        "the base URL {base} carries a query or fragment"
    );
    Ok(parsed.join(file_name)?.to_string())
}
