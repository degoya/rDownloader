//! The signed plugin repository index (RD-140-01): what a repository offers, and what it
//! withdraws.
//!
//! An index is a `rd_sign::SignedDocument` under [`PLUGIN_INDEX_DOMAIN`], signed by a
//! repository key — the compiled-in `Role::Repository` root for the official repository, a key
//! the person approved for any other. **A repository only delivers; it vouches for nothing.**
//! Every package it lists still has to verify under a trusted *plugin* key when it is
//! installed, exactly as a manually installed one does, and the index names each package by
//! the same `package_digest` the plugin signature covers (manifest, component, locales — not
//! the archive's file hash, which changes with the zip framing), so a downloaded package that
//! is not the one the index described is refused before anything else is looked at.
//!
//! Order matters, as in `rd_siterules::pack`: size, envelope, revocation and signature, schema
//! version, freshness, content. An index that fails at any step yields no entry at all. The
//! schema version is read *after* the signature, so an unsigned document declaring a future
//! version is reported as unsigned, not as "unknown version".
//!
//! Withdrawal travels in the index itself (owner, 2026-09-26): [`Revocations`] names package
//! digests and plugin signing keys that are no longer acceptable, and comes with the index's
//! signature and sequence, so a replayed older index cannot take a withdrawal back.

use chrono::{DateTime, Utc};
use rd_core::PluginId;
use rd_sign::{Role, SignedDocument, VerifyError, replay};
use serde::{Deserialize, Serialize};

use crate::{PluginType, parse_package_digest};

#[path = "index_build.rs"]
mod build;
#[path = "index_validate.rs"]
mod validate;

pub use build::{describe_package, package_url};

/// Domain separator for the index's signatures (`rd_sign::envelope`).
pub const PLUGIN_INDEX_DOMAIN: &str = "rdownloader.plugin-index.v1";

/// The index layout this build understands. Anything else is refused as a whole.
pub const PLUGIN_INDEX_SCHEMA_VERSION: u32 = 1;

/// Largest signed index accepted, checked before a byte of it is parsed. Roughly a thousand
/// entries with full release notes; the official index is a few dozen.
pub const MAX_INDEX_BYTES: usize = 4 * 1024 * 1024;
/// Most packages one index may list.
pub const MAX_INDEX_PACKAGES: usize = 512;
/// Longest release note per package, in characters. Plain text; the interface renders it as
/// text, never as markup.
pub const MAX_RELEASE_NOTES_CHARS: usize = 2000;
/// Most withdrawn package digests one index may carry.
pub const MAX_REVOKED_DIGESTS: usize = 4096;
/// Most withdrawn plugin signing keys one index may carry.
pub const MAX_REVOKED_KEYS: usize = 256;
/// Largest `.rdplug` an entry may declare: the component, manifest and locale ceilings the
/// archive reader enforces, plus room for the zip framing.
pub const MAX_PACKAGE_BYTES: u64 = 96 * 1024 * 1024;
/// How long an index built without an explicit window stays valid.
pub const DEFAULT_VALIDITY_DAYS: i64 = 90;
/// Longest window between `issued_at` and `not_after`. A replay of a genuine index is useful
/// to an attacker for exactly this long on a first contact.
pub const MAX_VALIDITY_DAYS: i64 = 366;

/// The signed payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginIndex {
    /// Always [`PLUGIN_INDEX_SCHEMA_VERSION`] for an index this build accepts.
    pub schema_version: u32,
    /// The publisher's monotonic counter, starting at 1. Never goes backwards; see
    /// [`rd_sign::replay`].
    pub sequence: u64,
    /// When the publisher says it signed this.
    pub issued_at: DateTime<Utc>,
    /// After this instant the index is stale. Required, unlike the tool manifest's: an index
    /// is fetched from the network on every refresh, and without an expiry a first contact
    /// would accept a replay of any genuine index ever published.
    pub not_after: DateTime<Utc>,
    /// Every package offered, at most one entry per plugin id and version.
    #[serde(default)]
    pub packages: Vec<IndexPackage>,
    /// What this repository withdraws.
    #[serde(default)]
    pub revoked: Revocations,
}

/// One downloadable package.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexPackage {
    pub id: PluginId,
    /// The manifest's default display name.
    pub name: String,
    /// The plugin's semantic version.
    pub version: String,
    /// The manifest's `plugin_type`. A type this build does not know is listed, not refused:
    /// the index is shared by every core version, and only the install decides.
    pub plugin_type: PluginType,
    /// The `rdownloader:plugin` contract version the component was built against; see
    /// [`contract`](Self::contract).
    pub api_version: String,
    /// Lowest core version the plugin runs on, from the manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_app_version: Option<String>,
    /// 64 lowercase hex characters of `package_digest` over manifest, component and locales:
    /// the value the plugin signature covers and plugin revocation is keyed by.
    pub package_digest: String,
    /// Byte size of the `.rdplug` file, so a download that grows past it is cut off early.
    pub size: u64,
    /// Absolute `https://` URL, or a path relative to the index's own URL that cannot leave
    /// the index's directory.
    pub url: String,
    /// Who signed the package.
    pub publisher: Publisher,
    /// What the package asks for, as the install preview shows it.
    pub permissions: Permissions,
    /// Plain-text notes for this version, at most [`MAX_RELEASE_NOTES_CHARS`] characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
}

/// The package's signer, as the manifest names it.
///
/// A claim until the package is downloaded. The repository writes the digest *and* this, so
/// matching the digest proves nothing about it; `PluginRepositoryService::download` compares it
/// with the key that actually signed the package before anything installs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Publisher {
    /// The manifest's `key_id`.
    pub key_id: String,
    /// Lowercase hex SHA-256 of the manifest's public key (`rd_sign::key_fingerprint`).
    pub fingerprint: String,
    /// The manifest's `metadata.author`.
    pub author: String,
}

/// The grants a package declares.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    /// `Capabilities::granted`, in its stable order.
    #[serde(default)]
    pub granted: Vec<String>,
    /// The `net_http` allowlist.
    #[serde(default)]
    pub http_domains: Vec<String>,
    /// The `net_stream` hosts.
    #[serde(default)]
    pub stream_hosts: Vec<String>,
}

impl Permissions {
    /// What `manifest` asks for, in the form an index entry and the install preview carry.
    #[must_use]
    pub fn of(manifest: &crate::PluginManifest) -> Self {
        let capabilities = &manifest.capabilities;
        Self {
            granted: capabilities.granted(),
            http_domains: capabilities.domains().to_vec(),
            stream_hosts: capabilities
                .net_stream
                .as_ref()
                .map(|stream| stream.hosts.clone())
                .unwrap_or_default(),
        }
    }

    /// Whether these permissions ask for anything `other` does not: a capability, a domain or a
    /// stream host. Order and duplicates do not count.
    #[must_use]
    pub fn widens(&self, other: &Self) -> bool {
        fn beyond(wanted: &[String], held: &[String]) -> bool {
            wanted.iter().any(|entry| !held.contains(entry))
        }
        beyond(&self.granted, &other.granted)
            || beyond(&self.http_domains, &other.http_domains)
            || beyond(&self.stream_hosts, &other.stream_hosts)
    }

    /// The same permissions, whatever order either lists them in.
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        !self.widens(other) && !other.widens(self)
    }

    /// What these permissions ask for that `held` does not, each list in this one's order: the
    /// part of an update the person has not granted yet (RD-160-09). Empty exactly when
    /// [`widens`](Self::widens) is false.
    #[must_use]
    pub fn beyond(&self, held: &Self) -> Self {
        fn missing(wanted: &[String], held: &[String]) -> Vec<String> {
            let mut missing: Vec<String> = Vec::new();
            for entry in wanted {
                if !held.contains(entry) && !missing.contains(entry) {
                    missing.push(entry.clone());
                }
            }
            missing
        }
        Self {
            granted: missing(&self.granted, &held.granted),
            http_domains: missing(&self.http_domains, &held.http_domains),
            stream_hosts: missing(&self.stream_hosts, &held.stream_hosts),
        }
    }

    /// Whether this asks for nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.granted.is_empty() && self.http_domains.is_empty() && self.stream_hosts.is_empty()
    }
}

/// What the repository withdraws.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Revocations {
    /// Withdrawn package digests, in the lowercase hex of [`IndexPackage::package_digest`].
    #[serde(default)]
    pub package_digests: Vec<String>,
    /// Withdrawn plugin signing keys.
    #[serde(default)]
    pub keys: Vec<RevokedKey>,
}

/// A withdrawn plugin signing key.
///
/// Named by id *and* fingerprint: an id alone is chosen by whoever writes the manifest, so a
/// repository withdrawing `some-author-v1` must not reach a different key that happens to
/// carry the same id on an installation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RevokedKey {
    pub key_id: String,
    pub fingerprint: String,
}

/// Why an index was refused. [`code`](Self::code) is what an interface or a log shows.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("the plugin index is larger than {MAX_INDEX_BYTES} bytes")]
    TooLarge,
    #[error("the plugin index cannot be read: {0}")]
    Malformed(String),
    #[error("the plugin index is not signed by a trusted repository key: {0}")]
    Untrusted(String),
    #[error("the plugin index's signature does not verify: {0}")]
    BadSignature(String),
    #[error("the plugin index has been revoked")]
    Revoked,
    #[error(
        "the plugin index declares schema version {saw}, this build reads {PLUGIN_INDEX_SCHEMA_VERSION}"
    )]
    SchemaVersion { saw: u64 },
    #[error("the plugin index is stale: {0}")]
    Stale(#[from] replay::StaleError),
    #[error("the plugin index is invalid: {0}")]
    Invalid(String),
}

impl IndexError {
    /// The stable code, translated by the interface.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLarge => "plugin_index.too_large",
            Self::Malformed(_) => "plugin_index.malformed",
            Self::Untrusted(_) => "plugin_index.untrusted",
            Self::BadSignature(_) => "plugin_index.bad_signature",
            Self::Revoked => "plugin_index.revoked",
            Self::SchemaVersion { .. } => "plugin_index.schema_version_unsupported",
            Self::Stale(_) => "plugin_index.stale",
            Self::Invalid(_) => "plugin_index.invalid",
        }
    }
}

impl PluginIndex {
    /// The freshness fields, as [`rd_sign::replay::check`] wants them.
    #[must_use]
    pub fn freshness(&self) -> replay::Freshness {
        replay::Freshness {
            sequence: self.sequence,
            issued_at: self.issued_at,
            not_after: Some(self.not_after),
        }
    }

    /// The withdrawn digests as bytes, ready for `PluginVerifier::revoke_package_digest`.
    pub fn revoked_package_digests(&self) -> anyhow::Result<Vec<[u8; 32]>> {
        self.revoked
            .package_digests
            .iter()
            .map(String::as_str)
            .map(parse_package_digest)
            .collect()
    }
}

impl IndexPackage {
    /// The contract this package was built against, as the WIT names it.
    #[must_use]
    pub fn contract(&self) -> String {
        format!("rdownloader:plugin@{}", self.api_version)
    }

    /// The digest a downloaded package has to match.
    pub fn digest(&self) -> anyhow::Result<[u8; 32]> {
        parse_package_digest(&self.package_digest)
    }

    /// Where to download this package, given the URL the index itself was fetched from.
    ///
    /// Always `https://`: a relative entry inherits the index's scheme and host, and an index
    /// fetched over anything else is refused here rather than turning into a plain-HTTP
    /// download.
    pub fn resolve_url(&self, index_url: &url::Url) -> anyhow::Result<url::Url> {
        let resolved = if self.url.starts_with("https://") {
            url::Url::parse(&self.url)?
        } else {
            index_url.join(&self.url)?
        };
        anyhow::ensure!(
            resolved.scheme() == "https",
            "package URL {resolved} is not https"
        );
        Ok(resolved)
    }
}

/// Verifies a signed index against the compiled-in repository root — the official repository.
///
/// `known_sequence` is the highest sequence this installation has already accepted *from this
/// repository*, which the caller persists; `None` only on a first contact.
pub fn verify(
    bytes: &[u8],
    known_sequence: Option<u64>,
    now: DateTime<Utc>,
) -> Result<PluginIndex, IndexError> {
    let trust = rd_sign::trust_store_for(Role::Repository, now)
        .map_err(|error| IndexError::Untrusted(error.to_string()))?;
    verify_with(bytes, &trust, known_sequence, now)
}

/// [`verify`] against an explicit trust store: a third-party repository's approved key, or a
/// test's.
pub fn verify_with(
    bytes: &[u8],
    trust: &rd_sign::TrustStore,
    known_sequence: Option<u64>,
    now: DateTime<Utc>,
) -> Result<PluginIndex, IndexError> {
    if bytes.len() > MAX_INDEX_BYTES {
        return Err(IndexError::TooLarge);
    }
    let document =
        SignedDocument::parse(bytes).map_err(|error| IndexError::Malformed(error.to_string()))?;
    let payload: serde_json::Value =
        document
            .verify(PLUGIN_INDEX_DOMAIN, trust)
            .map_err(|error| match error {
                VerifyError::UntrustedKey { .. } => IndexError::Untrusted(error.to_string()),
                VerifyError::BadSignature { .. } => IndexError::BadSignature(error.to_string()),
                VerifyError::Revoked => IndexError::Revoked,
                VerifyError::Other(other) => IndexError::Malformed(other.to_string()),
            })?;
    let saw = payload
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| IndexError::Malformed("schema_version is missing".to_owned()))?;
    if saw != u64::from(PLUGIN_INDEX_SCHEMA_VERSION) {
        return Err(IndexError::SchemaVersion { saw });
    }
    let index: PluginIndex = serde_json::from_value(payload)
        .map_err(|error| IndexError::Malformed(error.to_string()))?;
    replay::check(index.freshness(), known_sequence, now)?;
    index.validate()?;
    Ok(index)
}

/// Signs `index` under `key_id` and returns the document as it is published.
///
/// Refuses an index [`verify_with`] would refuse for its content or size, so a broken index
/// never leaves the publisher.
pub fn sign(
    key_id: &str,
    key: &rd_sign::SigningKey,
    index: &PluginIndex,
) -> anyhow::Result<Vec<u8>> {
    index.validate()?;
    let document = rd_sign::sign_document(PLUGIN_INDEX_DOMAIN, key_id, key, index)?;
    let bytes = serde_json::to_vec_pretty(&document)?;
    anyhow::ensure!(
        bytes.len() <= MAX_INDEX_BYTES,
        "the signed index is {} bytes, more than the {MAX_INDEX_BYTES} a reader accepts",
        bytes.len()
    );
    Ok(bytes)
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;
