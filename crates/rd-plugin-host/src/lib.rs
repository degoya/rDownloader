//! Signed resolver package validation and atomic installation.

mod account_label;
pub mod artifact;
mod bundled;
mod component;
mod conformance;
mod diagnostics;
pub mod extension;
mod foreign_address;
mod foreign_text;
mod installed;
pub mod keyderive;
mod locales;
mod manifest;
mod native;
mod packager;
mod registry;
mod revocation;
mod runtime;
mod session;
mod siterules;
mod transfer;

pub use bundled::{BundledSyncReport, sync_bundled};
pub use component::ComponentResolver;
pub use conformance::{ConformanceCheck, ConformanceReport, check_package};
pub use diagnostics::{ExecutionLog, ExecutionOutcome, Invocation};
pub use installed::IncompatiblePlugin;
pub use locales::{
    MAX_LOCALE_BYTES, MAX_LOCALE_FILES, PluginLocale, PluginLocaleAccount, locale_member_language,
    parse_locale, valid_language, validate_locales,
};
pub use manifest::{
    Capabilities, CredentialKindManifest, MANIFEST_VERSION, ManifestHeader, ManifestRejection,
    NetHttpCapability, OAuthFlowManifest, PluginManifest, PluginMetadata, PluginType,
    ProviderKindManifest, ProviderManifest, SUPPORTED_API_VERSIONS, SecretFilledByManifest,
    check_app_version, decode_public_key, provider_spec_from_manifest,
};
pub use native::{
    CLIENT_ID_MARKER, ResolverService, client_not_configured, provider_cookie_scope,
    provider_download_authorization, provider_download_bearer,
    provider_download_carries_credential, provider_token_beside_the_flow,
};
pub use packager::{
    GeneratedKey, generate_signing_key, load_signing_key_pem, package_plugin, public_key_base64,
};
pub use rd_plugin_api::{AccountStatus, LabelPart};
pub use registry::PluginTypeRegistry;
pub use revocation::{RevokedDigests, format_package_digest, parse_package_digest};
pub use runtime::{PluginStoreState, SandboxEngine};
pub use siterules::{RuleCaptcha, RuleFetcher, RuleNetwork, RuleResolver};
pub use transfer::{
    RemoteFile, TransferBackend, TransferJob, TransferOutcome, TransferState, TransferTarget,
};

use std::{
    io::{Cursor, Read, Seek},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ed25519_dalek::VerifyingKey;
use wasmparser::Validator;

use manifest::{safe_segment, validate_manifest};

/// Core version a plugin's `metadata.min_app_version` is compared against.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_COMPONENT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 1024;

const MANIFEST_MEMBER: &str = "manifest.toml";
const COMPONENT_MEMBER: &str = "component.wasm";
const SIGNATURE_MEMBER: &str = "signature.ed25519";

/// Default instruction budget per invocation. Parsing a single ~200 KiB HTML
/// page costs a few million instructions, so the budget is generous; the epoch
/// timeout remains the wall-clock guard.
pub const DEFAULT_FUEL: u64 = 2_000_000_000;

/// Resource limits applied by the component runtime.
#[derive(Clone, Copy, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub struct PluginLimits {
    pub memory_bytes: u64,
    pub fuel: u64,
    pub timeout_milliseconds: u64,
    pub max_response_bytes: u64,
    /// Total time a resolver may spend in host-side waits (hoster countdowns) per
    /// invocation. Separate from `timeout_milliseconds`, which bounds compute time only.
    #[serde(default = "default_wait_budget")]
    pub wait_budget_milliseconds: u64,
}

/// Free downloads routinely wait a few minutes; ten minutes covers a countdown plus the
/// captcha round trip without letting a broken plugin occupy a slot indefinitely.
const fn default_wait_budget() -> u64 {
    10 * 60 * 1000
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 * 1024 * 1024,
            fuel: DEFAULT_FUEL,
            timeout_milliseconds: 15_000,
            max_response_bytes: 8 * 1024 * 1024,
            wait_budget_milliseconds: default_wait_budget(),
        }
    }
}

/// One installed package: where it landed, and the manifest that was installed.
///
/// Callers need the manifest to register the plugin's provider row, so returning it removes
/// any need to guess which of the installed manifests the new path belongs to.
pub struct InstalledPackage {
    pub path: PathBuf,
    pub manifest: PluginManifest,
}

/// Validated archive contents before installation.
pub struct VerifiedPackage {
    pub manifest: PluginManifest,
    pub manifest_bytes: Vec<u8>,
    pub component: Vec<u8>,
    pub signature: Option<Vec<u8>>,
    /// `(language, raw JSON)` pairs, sorted by language, exactly as signed.
    pub locales: Vec<(String, Vec<u8>)>,
}

/// Why a package was refused, separating the recoverable trust decision from hard errors.
#[derive(Debug)]
pub enum VerifyError {
    /// The package is internally consistent and self-signed by a key the user has not
    /// yet trusted. The caller may show the fingerprint and ask for confirmation.
    UntrustedKey {
        key_id: String,
        /// Base64 Ed25519 key the package declares, ready to be recorded on confirmation.
        public_key: String,
        fingerprint: String,
        name: String,
        version: String,
    },
    Other(anyhow::Error),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UntrustedKey {
                key_id,
                fingerprint,
                ..
            } => write!(
                formatter,
                "untrusted signing key {key_id} (fingerprint {fingerprint})"
            ),
            Self::Other(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for VerifyError {}

impl From<anyhow::Error> for VerifyError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

pub use rd_sign::key_fingerprint;

/// Trust store and policy for resolver packages.
///
/// The key map is shared so a key confirmed at runtime (trust on first use) is visible to
/// every clone, including the installer held by the API layer.
#[derive(Clone, Default)]
pub struct PluginVerifier {
    trusted_keys: rd_sign::TrustStore,
    /// Withdrawn package digests, shared with every clone exactly as the key map is.
    revoked: RevokedDigests,
    development_mode: bool,
}

impl PluginVerifier {
    /// Creates a verifier; unsigned packages are allowed only in development mode.
    #[must_use]
    pub fn new(development_mode: bool) -> Self {
        Self {
            trusted_keys: rd_sign::TrustStore::new(),
            revoked: RevokedDigests::new(),
            development_mode,
        }
    }

    /// Adds an explicitly trusted Ed25519 public key.
    pub fn trust_key(&self, key_id: String, key: VerifyingKey) -> Result<()> {
        self.trusted_keys
            .trust(key_id, key)
            .context("trust a plugin signing key")
    }

    /// Imports one base64-encoded 32-byte Ed25519 verification key.
    pub fn trust_key_base64(&self, key_id: String, encoded: &str) -> Result<()> {
        self.trust_key(key_id, decode_public_key(encoded)?)
    }

    /// Drops a key so newly installed packages signed with it are refused again.
    pub fn revoke_key(&self, key_id: &str) -> Result<bool> {
        self.trusted_keys.revoke(key_id)
    }

    /// Withdraws one exact package by its content digest, leaving its signing key trusted.
    ///
    /// This is the second axis of plugin trust and the one a published-then-withdrawn version
    /// needs: revoking the release key instead would take down every other plugin the same
    /// author ever signed, which is a far larger blast radius than the problem. The digest is
    /// `package_digest` over the archive's members, so it names one version's exact bytes and
    /// nothing else.
    ///
    /// Returns whether the digest was not already withdrawn, so the caller can tell a
    /// withdrawal apart from a repeat of one. A plugin already running is left alone: the
    /// refusal takes effect the next time the package is loaded, exactly as a revoked key
    /// does. Nothing here persists the list — `set_revoked_package_digests` seeds it from
    /// whatever the caller stored.
    pub fn revoke_package_digest(&self, digest: [u8; 32]) -> Result<bool> {
        self.revoked.insert(digest)
    }

    /// Takes a withdrawal back; returns whether the digest was withdrawn at all.
    pub fn unrevoke_package_digest(&self, digest: &[u8; 32]) -> Result<bool> {
        self.revoked.remove(digest)
    }

    /// Seeds the withdrawn set from persisted state, replacing whatever is held now.
    ///
    /// Called once at startup with the stored digests, the way the confirmed signing keys are
    /// restored. Without it the set is empty after every restart and a withdrawn version
    /// quietly loads again — the whole list is in-process state, and a process that has just
    /// started has none of it.
    pub fn set_revoked_package_digests(
        &self,
        digests: impl IntoIterator<Item = [u8; 32]>,
    ) -> Result<()> {
        self.revoked.replace(digests)
    }

    /// Whether this exact package has been withdrawn.
    pub fn is_package_revoked(&self, digest: &[u8; 32]) -> Result<bool> {
        self.revoked.contains(digest)
    }

    /// Every withdrawn digest in hex, for a listing that has to agree with what is stored.
    pub fn revoked_package_digests(&self) -> Result<Vec<String>> {
        Ok(self
            .revoked
            .all()?
            .iter()
            .map(format_package_digest)
            .collect())
    }

    /// Whether `key_id` is currently trusted.
    pub fn is_trusted(&self, key_id: &str) -> Result<bool> {
        self.trusted_keys.is_trusted(key_id)
    }

    /// Reads, validates and verifies one resolver archive.
    pub fn verify_file(&self, path: &Path) -> Result<VerifiedPackage, VerifyError> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("open plugin package {}", path.display()))?;
        self.verify_reader(file)
    }

    /// Validates an in-memory package received through the local REST API.
    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<VerifiedPackage, VerifyError> {
        self.verify_reader(Cursor::new(bytes))
    }

    fn verify_reader<R: Read + Seek>(&self, reader: R) -> Result<VerifiedPackage, VerifyError> {
        let (manifest_bytes, component, signature, locales) = read_archive(reader)?;
        self.verify_parts(manifest_bytes, component, signature, locales)
    }

    pub(crate) fn verify_parts(
        &self,
        manifest_bytes: Vec<u8>,
        component: Vec<u8>,
        signature: Option<Vec<u8>>,
        locales: Vec<(String, Vec<u8>)>,
    ) -> Result<VerifiedPackage, VerifyError> {
        let package = self.verify_contents(manifest_bytes, component, signature, locales)?;
        validate_component(&package.component)?;
        SandboxEngine::new(package.manifest.limits)?
            .compile_component(&package.component, &package.manifest)
            .context("compile and validate plugin component imports")?;
        Ok(package)
    }

    /// Everything the signature decides, for the paths that read a package without running it.
    ///
    /// The provider rows and the interface's locale bundle are built from the manifest and the
    /// locale files alone, and both used to read them straight off disk with no signature check
    /// at all: a manifest edited in place still contributed its `request_domains`, its cookie
    /// scope and its secret slots to the registry, and its locale JSON to the bundle, while the
    /// very same package was skipped as "no longer verifies" the moment its component was
    /// loaded. The digest covers the component too, so those bytes are still read and hashed
    /// here; only the wasmparser validation and the compile are left out, because neither of
    /// those two paths runs any guest code. What this returns is therefore not known to be
    /// loadable — only `verify_parts` hands out a package that is.
    pub(crate) fn verify_contents(
        &self,
        manifest_bytes: Vec<u8>,
        component: Vec<u8>,
        signature: Option<Vec<u8>>,
        locales: Vec<(String, Vec<u8>)>,
    ) -> Result<VerifiedPackage, VerifyError> {
        let manifest: PluginManifest = toml::from_slice(&manifest_bytes)
            .context("parse plugin manifest")
            .map_err(VerifyError::Other)?;
        validate_manifest(&manifest)?;
        // A plugin that needs a newer core is refused here rather than half-loaded: the
        // manifest revision alone cannot express "this build is too old", and without the
        // check `metadata.min_app_version` would be documentation nobody enforces.
        check_app_version(&manifest, APP_VERSION).map_err(VerifyError::Other)?;
        validate_locales(manifest.message_slug(), &locales)?;

        // Computed once: the signature is checked against it and the content revocation
        // list is keyed by it. The framing must stay the one every released `.rdplug` was
        // signed with.
        let digest = package_digest(&manifest_bytes, &component, &locales);

        // Signature before anything touches the component bytes. The manifest and the
        // locales are small and bounded; the component is not, and `install_bytes` accepts
        // up to 64 MiB from the REST path before anyone has proved who sent them. Running
        // `validate_component` first handed all of it to the wasmparser validator
        // pre-authentication, so the parser was reachable by anybody who could post an
        // archive. A package from an unknown key stops here too, as a trust decision, with
        // its component still unparsed and uncompiled — `verify_parts` runs the validator
        // only after this has returned.

        match signature.as_deref() {
            Some(encoded) => self.verify_signed(&manifest, &digest, encoded)?,
            None if !self.development_mode => {
                return Err(VerifyError::Other(anyhow::anyhow!(
                    "unsigned plugins require development mode"
                )));
            }
            None => {
                tracing::warn!(plugin = %manifest.name, "accepting unsigned development plugin")
            }
        }

        // A withdrawn version is refused after its signature has checked out, not instead of
        // it: the two answer different questions, and a tampered package has to say that
        // rather than be reported as revoked. Every path that accepts a package comes through
        // here — install and the re-verification done at every start — so a revoked digest
        // takes the plugin out of the next load exactly as a revoked key does.
        if self.revoked.contains(&digest)? {
            return Err(VerifyError::Other(anyhow::anyhow!(
                "plugin package {} {} was withdrawn by its content digest",
                manifest.name,
                manifest.version
            )));
        }

        Ok(VerifiedPackage {
            manifest,
            manifest_bytes,
            component,
            signature,
            locales,
        })
    }

    /// Verifies the signature, distinguishing an unknown author from a bad package.
    ///
    /// A manifest always carries its author's public key, so an unknown `key_id` can still
    /// be checked for internal consistency: only a package that genuinely proves possession
    /// of that key reaches the user as a trust prompt. A known `key_id` is pinned to the key
    /// already stored for it, so a package cannot claim someone else's key id.
    fn verify_signed(
        &self,
        manifest: &PluginManifest,
        digest: &[u8; 32],
        encoded: &[u8],
    ) -> Result<(), VerifyError> {
        let declared = manifest.verifying_key()?;
        let trusted = self.trusted_keys.key(&manifest.key_id)?;
        match trusted {
            Some(trusted) if trusted == declared => {
                verify_signature(&trusted, digest, encoded)?;
                Ok(())
            }
            Some(_) => Err(VerifyError::Other(anyhow::anyhow!(
                "plugin public_key does not match the trusted key for {}",
                manifest.key_id
            ))),
            None => {
                verify_signature(&declared, digest, encoded)?;
                Err(VerifyError::UntrustedKey {
                    key_id: manifest.key_id.clone(),
                    public_key: manifest.public_key.clone(),
                    fingerprint: key_fingerprint(&declared),
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                })
            }
        }
    }
}

/// Reads the four known member kinds and refuses anything else in the archive.
type ArchiveParts = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Vec<(String, Vec<u8>)>);

fn read_archive<R: Read + Seek>(reader: R) -> Result<ArchiveParts> {
    let mut archive = zip::ZipArchive::new(reader).context("read .rdplug archive")?;
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let mut languages = Vec::new();
    for name in &names {
        match name.as_str() {
            MANIFEST_MEMBER | COMPONENT_MEMBER | SIGNATURE_MEMBER => {}
            other => match locale_member_language(other) {
                Some(language) => languages.push(language.to_owned()),
                None => bail!("unexpected member {other} in .rdplug archive"),
            },
        }
    }
    if languages.len() > MAX_LOCALE_FILES {
        bail!("plugin ships more than {MAX_LOCALE_FILES} locale files");
    }
    languages.sort();
    let manifest_bytes = read_member(&mut archive, MANIFEST_MEMBER, MAX_MANIFEST_BYTES)?;
    let component = read_member(&mut archive, COMPONENT_MEMBER, MAX_COMPONENT_BYTES)?;
    let signature = read_optional_member(&mut archive, SIGNATURE_MEMBER, MAX_SIGNATURE_BYTES)?;
    let mut locales = Vec::with_capacity(languages.len());
    for language in languages {
        let member = format!("locales/{language}.json");
        let bytes = read_member(&mut archive, &member, MAX_LOCALE_BYTES)?;
        locales.push((language, bytes));
    }
    Ok((manifest_bytes, component, signature, locales))
}

/// Atomically installs packages into `<root>/<plugin-id>/<version>`.
#[derive(Clone)]
pub struct PluginInstaller {
    root: PathBuf,
    verifier: PluginVerifier,
    /// Plugin ids the user switched off. Shared with every clone, because the installer is
    /// cloned into each subsystem that loads plugins.
    disabled: std::sync::Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
}

impl PluginInstaller {
    #[must_use]
    pub fn new(root: PathBuf, verifier: PluginVerifier) -> Self {
        Self {
            root,
            verifier,
            disabled: std::sync::Arc::default(),
        }
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
        let staging = plugin_root.join(format!(".install-{}", rd_core::PluginId::new()));
        tokio::fs::create_dir(&staging).await?;
        if let Err(error) = write_staging(&staging, &package).await {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(error);
        }
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

/// Structural WebAssembly validation, before an engine is created for the component.
///
/// No byte scan for `wasi:`, `wasi_snapshot_preview1` or `env` any more: a scan cannot tell an
/// import from a data-section string, so a plugin that merely carried `wasi:` in its strings
/// was refused as though it imported it — the same false positive `check-plugin-imports.sh`
/// records for its own fallback scan. The authoritative check is the import walk in
/// `SandboxEngine::compile_component`, which runs on every path this one does and refuses any
/// import the manifest did not grant. Nothing escapes it: a component cannot leave a core
/// module's `env` import unsatisfied, and a `wasi:` world appears there as an import like any
/// other.
fn validate_component(component: &[u8]) -> Result<()> {
    Validator::new()
        .validate_all(component)
        .context("invalid WebAssembly component")?;
    Ok(())
}

fn verify_signature(key: &VerifyingKey, digest: &[u8; 32], encoded_signature: &[u8]) -> Result<()> {
    rd_sign::verify_detached_bytes(key, digest, encoded_signature)
        .context("plugin signature verification failed")
}

/// Computes the exact payload signed by plugin release tooling.
///
/// Every field is length-prefixed so member boundaries cannot be shifted, and the locale
/// files are folded in by name so translations are as tamper-evident as the component.
#[must_use]
pub fn package_digest(
    manifest: &[u8],
    component: &[u8],
    locales: &[(String, Vec<u8>)],
) -> [u8; 32] {
    let mut builder = rd_sign::DigestBuilder::new();
    builder.field(manifest);
    builder.field(component);
    builder.named_fields(locales);
    builder.finish()
}

fn read_member<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    read_optional_member(archive, name, limit)?.with_context(|| format!("missing {name}"))
}

fn read_optional_member<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>> {
    let member = match archive.by_name(name) {
        Ok(member) => member,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if member.size() > limit {
        bail!("plugin member {name} exceeds its size limit");
    }
    let mut bytes = Vec::with_capacity(member.size() as usize);
    member.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("plugin member {name} exceeds its size limit");
    }
    Ok(Some(bytes))
}

/// Checks a target and every redirect against an exact or wildcard domain allowlist.
pub fn domain_allowed(url: &url::Url, domains: &[String]) -> bool {
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && domains.iter().any(|domain| {
            domain == "*"
                || domain
                    .strip_prefix("*.")
                    .is_some_and(|suffix| host.ends_with(&format!(".{suffix}")))
                || host == *domain
        })
}

/// Refuses a widget captcha whose page lies outside the plugin's declared domains.
///
/// A widget challenge is the one host call that ends with a *person* looking at a hoster
/// page, so letting a plugin name any page at all would turn `solve-captcha` into a way to
/// put arbitrary content in front of the user. The boundary is the manifest's, the same one
/// `net_http` is held to (RD-107-03).
pub(crate) fn captcha_target_refused() -> rd_core::Failure {
    rd_core::Failure::coded(
        rd_core::FailureKind::Permanent,
        "plugin.captcha_target_not_allowed",
        "Captcha page is outside the plugin's declared domains",
    )
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::{
        PluginInstaller, PluginVerifier, VerifyError, domain_allowed, key_fingerprint,
        package_digest,
    };

    pub(crate) const TEST_PUBLIC_KEY: &str = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY=";

    /// A complete v2 manifest for tests; `extra` is appended to the `[provider]` table.
    pub(crate) fn fixture_manifest(public_key: &str) -> String {
        format!(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-00000000abcd"
name = "Fixture"
version = "1.2.3"
key_id = "fixture-v1"
public_key = "{public_key}"
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "A fixture resolver"
author = "Fixture Author"

[provider]
slug = "fixture"
kind = "hoster"
credentials = "api_key"
"#
        )
    }

    #[test]
    fn release_signature_payload_is_stable() {
        let signing = SigningKey::from_bytes(&[7_u8; 32]);
        let digest = package_digest(b"manifest", b"component", &[]);
        let signature = signing.sign(&digest);
        assert!(
            signing
                .verifying_key()
                .verify_strict(&digest, &signature)
                .is_ok()
        );
    }

    #[test]
    fn locale_files_are_covered_by_the_digest() {
        let base = package_digest(b"manifest", b"component", &[]);
        let with_locale = package_digest(
            b"manifest",
            b"component",
            &[("en".to_owned(), b"{}".to_vec())],
        );
        assert_ne!(base, with_locale, "locales must change the signed payload");

        // Order of the input slice must not matter; the digest sorts by language.
        let ascending = package_digest(
            b"manifest",
            b"component",
            &[
                ("de".to_owned(), b"{\"a\":1}".to_vec()),
                ("en".to_owned(), b"{\"b\":2}".to_vec()),
            ],
        );
        let descending = package_digest(
            b"manifest",
            b"component",
            &[
                ("en".to_owned(), b"{\"b\":2}".to_vec()),
                ("de".to_owned(), b"{\"a\":1}".to_vec()),
            ],
        );
        assert_eq!(ascending, descending);
    }

    #[test]
    fn fingerprints_are_stable_hex_sha256() {
        let key = SigningKey::from_bytes(&[3_u8; 32]).verifying_key();
        let fingerprint = key_fingerprint(&key);
        assert_eq!(fingerprint.len(), 64);
        assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(fingerprint, key_fingerprint(&key));
    }

    #[test]
    fn trust_store_is_shared_between_clones() {
        let verifier = PluginVerifier::new(false);
        let clone = verifier.clone();
        assert!(!clone.is_trusted("fixture-v1").expect("read"));
        verifier
            .trust_key_base64("fixture-v1".to_owned(), TEST_PUBLIC_KEY)
            .expect("trust");
        assert!(
            clone.is_trusted("fixture-v1").expect("read"),
            "runtime trust must reach existing clones"
        );
        assert!(clone.revoke_key("fixture-v1").expect("revoke"));
        assert!(!verifier.is_trusted("fixture-v1").expect("read"));
    }

    #[test]
    fn redirects_must_stay_inside_manifest_domains() {
        let domains = vec!["api.premiumize.me".to_owned(), "*.ddownload.com".to_owned()];
        assert!(domain_allowed(
            &"https://api.premiumize.me/api/account/info"
                .parse()
                .expect("valid URL"),
            &domains
        ));
        assert!(domain_allowed(
            &"https://cdn.ddownload.com/file".parse().expect("valid URL"),
            &domains
        ));
        assert!(!domain_allowed(
            &"https://ddownload.com.attacker.invalid/"
                .parse()
                .expect("valid URL"),
            &domains
        ));
    }

    #[tokio::test]
    async fn installed_manifests_are_discovered_without_loading_components() {
        let directory = tempfile::tempdir().expect("tempdir");
        let manifest_bytes = fixture_manifest(TEST_PUBLIC_KEY);
        let manifest: super::PluginManifest =
            toml::from_str(&manifest_bytes).expect("manifest parses");
        let version = directory
            .path()
            .join(manifest.id.to_string())
            .join(&manifest.version);
        std::fs::create_dir_all(&version).expect("plugin directory");
        std::fs::write(version.join("manifest.toml"), &manifest_bytes).expect("manifest file");
        let installer =
            PluginInstaller::new(directory.path().to_owned(), PluginVerifier::new(false));

        let installed = installer.list_installed().await.expect("installed plugins");
        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id, manifest.id);
        assert_eq!(installed[0].version, "1.2.3");
        assert_eq!(installed[0].message_slug(), "fixture");
    }

    #[test]
    fn untrusted_key_is_reported_separately_from_a_broken_package() {
        let error = VerifyError::UntrustedKey {
            key_id: "third-party".to_owned(),
            public_key: TEST_PUBLIC_KEY.to_owned(),
            fingerprint: "ab".repeat(32),
            name: "Fixture".to_owned(),
            version: "1.0.0".to_owned(),
        };
        assert!(error.to_string().contains("third-party"));
        assert!(matches!(error, VerifyError::UntrustedKey { .. }));
    }
}
