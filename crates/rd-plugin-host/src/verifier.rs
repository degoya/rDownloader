//! `PluginVerifier`: the trust store and the checks a package passes before anything runs it --
//! reading the archive with bounded members, the manifest, the locales, the signature against
//! the content digest, the withdrawal lists, and the WebAssembly validation.
//!
//! Split out of `lib.rs` (PLUG-21).

use std::{
    io::{Cursor, Read, Seek},
    path::Path,
};

use anyhow::{Context, Result, bail};
use ed25519_dalek::VerifyingKey;
use wasmparser::Validator;

use crate::{
    APP_VERSION, COMPONENT_MEMBER, MANIFEST_MEMBER, MAX_COMPONENT_BYTES, MAX_LOCALE_BYTES,
    MAX_LOCALE_FILES, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES, PluginManifest, RevokedDigests,
    SIGNATURE_MEMBER, SandboxEngine, VerifiedPackage, VerifyError, WithdrawnKeys,
    check_app_version, decode_public_key, format_package_digest, key_fingerprint,
    locale_member_language, manifest::validate_manifest, unsigned_notice, validate_locales,
};

/// Trust store and policy for resolver packages.
///
/// The key map is shared so a key confirmed at runtime (trust on first use) is visible to
/// every clone, including the installer held by the API layer.
#[derive(Clone, Default)]
pub struct PluginVerifier {
    pub(crate) trusted_keys: rd_sign::TrustStore,
    /// Withdrawn package digests, shared with every clone exactly as the key map is.
    pub(crate) revoked: RevokedDigests,
    /// Fingerprints of signing keys a repository index withdrew (RD-140-01). Checked before the
    /// trust decision, so a withdrawn key is refused whether or not anybody trusted it.
    pub(crate) withdrawn_keys: WithdrawnKeys,
    pub(crate) development_mode: bool,
}

impl PluginVerifier {
    /// Creates a verifier; unsigned packages are allowed only in development mode.
    #[must_use]
    pub fn new(development_mode: bool) -> Self {
        Self {
            trusted_keys: rd_sign::TrustStore::new(),
            revoked: RevokedDigests::new(),
            withdrawn_keys: WithdrawnKeys::default(),
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

    /// Withdraws a signing key by its fingerprint; returns whether it was not withdrawn yet.
    ///
    /// Stronger than [`revoke_key`](Self::revoke_key): that drops a trusted key and leaves the
    /// next package signed with it to ask for trust again, while a withdrawn key is refused
    /// outright, trust prompt included. Named by fingerprint, so no other key that happens to
    /// carry the same id is touched.
    pub fn withdraw_key(&self, fingerprint: &str) -> Result<bool> {
        self.withdrawn_keys.insert(fingerprint)
    }

    /// Seeds the withdrawn keys from persisted state, replacing whatever is held now.
    pub fn set_withdrawn_keys(&self, fingerprints: impl IntoIterator<Item = String>) -> Result<()> {
        self.withdrawn_keys.replace(fingerprints)
    }

    /// Whether the key with this fingerprint was withdrawn.
    pub fn is_key_withdrawn(&self, fingerprint: &str) -> Result<bool> {
        self.withdrawn_keys.contains(fingerprint)
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
            None => unsigned_notice::accepted(&manifest.name, &manifest.version),
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
        if self.withdrawn_keys.contains(&key_fingerprint(&declared))? {
            return Err(VerifyError::Other(anyhow::anyhow!(
                "plugin signing key {} was withdrawn by a repository",
                manifest.key_id
            )));
        }
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

pub(crate) fn read_archive<R: Read + Seek>(reader: R) -> Result<ArchiveParts> {
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
pub(crate) fn validate_component(component: &[u8]) -> Result<()> {
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
