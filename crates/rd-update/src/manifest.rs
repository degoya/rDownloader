//! The signed update manifest (RD-180-01): which version a channel offers, and the bytes it is
//! made of.
//!
//! A manifest is a `rd_sign::SignedDocument` under [`UPDATE_MANIFEST_DOMAIN`], signed by the
//! compiled-in `Role::Release` root (`rdownloader-update-v1`). The release workflow builds one per
//! release from the published `SHA256SUMS` and the version's `CHANGELOG.md` section and attaches
//! it as [`Channel::file_name`]: `rdownloader-update-stable.json` to a plain `vX.Y.Z` release,
//! `rdownloader-update-beta.json` to a `vX.Y.Z-beta.N` pre-release.
//!
//! The order of the checks follows `rd_plugin_host::index`: size, envelope, revocation and
//! signature, schema version, channel, freshness, content. The schema version is read *after*
//! the signature, so an unsigned document declaring a future version is reported as unsigned.
//! The channel is part of the signed payload and compared with the channel the manifest was
//! fetched for, so a genuine beta manifest served at the stable address is refused rather than
//! offered to a stable installation.
//!
//! **Additive fields are tolerated, incompatible changes bump the schema.** Neither the manifest
//! nor an artifact refuses a field it does not know: a later release that adds one must not
//! break the update check of every installation already in the field, which is what the check
//! is for. A change an older reader would misread raises [`UPDATE_MANIFEST_SCHEMA_VERSION`],
//! which that reader refuses as a whole (`update.schema_version_unsupported`). The strict parts
//! stay strict: the signature, its domain, the channel, freshness and the content rules.
//!
//! Every `sha256` and `size` here is what [`crate::download_verified`] holds a download to.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use rd_sign::{Role, SignedDocument, TrustStore, VerifyError, replay};
use serde::{Deserialize, Serialize};

/// Domain separator for the manifest's signatures (`rd_sign::envelope`).
pub const UPDATE_MANIFEST_DOMAIN: &str = "rdownloader.update-manifest.v1";
/// The manifest layout this build understands. Anything else is refused as a whole.
pub const UPDATE_MANIFEST_SCHEMA_VERSION: u32 = 1;
/// Largest signed manifest accepted, checked before a byte of it is parsed.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;
/// Longest release note, in characters. Plain text; the interface renders it as text.
pub const MAX_NOTES_CHARS: usize = 8000;
/// Most artifacts one manifest may list: five archives and a handful of installers today.
pub const MAX_ARTIFACTS: usize = 64;
/// Largest artifact a manifest may declare. The biggest archive is ~60 MB.
pub const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// How long a manifest built without an explicit window stays valid. Long, because a stable
/// release can stay the newest for months, and an expired manifest turns every installation's
/// check into an error; a replay of a genuine manifest can only hide a newer version, never
/// install an older one, since nothing below the running version is ever offered.
pub const DEFAULT_VALIDITY_DAYS: i64 = 180;
/// Longest window between `issued_at` and `not_after`.
pub const MAX_VALIDITY_DAYS: i64 = 366;

/// Which releases an installation is offered.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Plain `vX.Y.Z` releases only.
    #[default]
    Stable,
    /// `vX.Y.Z-beta.N` pre-releases as well as the stable releases.
    Beta,
}

impl std::fmt::Display for Channel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Channel {
    /// The stable name used in settings, the manifest and the interface.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }

    /// The channel `value` names, if any.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stable" => Some(Self::Stable),
            "beta" => Some(Self::Beta),
            _ => None,
        }
    }

    /// The release asset this channel's manifest is published as.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Stable => "rdownloader-update-stable.json",
            Self::Beta => "rdownloader-update-beta.json",
        }
    }
}

/// The signed payload. Unknown fields are ignored; see the module documentation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateManifest {
    /// Always [`UPDATE_MANIFEST_SCHEMA_VERSION`] for a manifest this build accepts.
    pub schema_version: u32,
    /// The publisher's monotonic counter for this channel; the release workflow uses the signing
    /// time in Unix seconds. Never goes backwards; see [`rd_sign::replay`].
    pub sequence: u64,
    /// When the publisher says it signed this.
    pub issued_at: DateTime<Utc>,
    /// After this instant the manifest is stale. Required, as in the plugin index: without it a
    /// first contact would accept a replay of any manifest ever published.
    pub not_after: DateTime<Utc>,
    /// The channel this manifest is published on.
    pub channel: Channel,
    /// The offered version, SemVer without a leading `v`. A pre-release on the beta channel
    /// only, and only there.
    pub version: String,
    /// When the release was published.
    pub released_at: DateTime<Utc>,
    /// Short plain-text release notes, at most [`MAX_NOTES_CHARS`] characters.
    #[serde(default)]
    pub notes: String,
    /// The downloadable files, at most one per platform, architecture and kind.
    pub artifacts: Vec<Artifact>,
    /// Whether this version's database migrations differ from the release before it on its
    /// channel (RD-180-02, RD-180-03): the release workflow compares `crates/rd-db/migrations/`
    /// with the previous plain tag, or the previous beta for a beta. Absent reads as `true`
    /// ([`Self::changes_schema`]), the side that asks for the encrypted backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_change: Option<bool>,
}

/// One downloadable file of a release.
///
/// `platform`, `arch` and `kind` are open strings rather than enums on purpose: a later release
/// that adds a platform or an installer kind must not make every older installation refuse the
/// whole manifest. What this build does not know it simply never selects; an unknown field it
/// ignores.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artifact {
    /// `linux`, `windows` or `macos`.
    pub platform: String,
    /// `x86_64` or `aarch64`.
    pub arch: String,
    /// `archive` (the portable `.tar.gz`/`.zip`), `msi`, `deb` or `rpm`.
    pub kind: String,
    /// Absolute `https://` URL.
    pub url: String,
    /// 64 lowercase hex characters.
    pub sha256: String,
    /// Exact byte size.
    pub size: u64,
}

/// The artifact kinds this build knows.
pub mod kind {
    pub const ARCHIVE: &str = "archive";
    pub const MSI: &str = "msi";
    pub const DEB: &str = "deb";
    pub const RPM: &str = "rpm";
}

/// Why a manifest, a check or a download was refused. [`code`](Self::code) is the stable code
/// the interface translates.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("this build carries no update signing key; the update check is not configured")]
    NotConfigured,
    #[error("no update manifest is published for this channel yet")]
    NotPublished,
    #[error("the update manifest could not be fetched: {0}")]
    Fetch(String),
    #[error("the update manifest is larger than {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("the update manifest cannot be read: {0}")]
    Malformed(String),
    #[error("the update manifest is not signed by a trusted update key: {0}")]
    Untrusted(String),
    #[error("the update manifest's signature does not verify: {0}")]
    BadSignature(String),
    #[error("the update manifest has been revoked")]
    Revoked,
    #[error(
        "the update manifest declares schema version {saw}, this build reads {UPDATE_MANIFEST_SCHEMA_VERSION}"
    )]
    SchemaVersion { saw: u64 },
    #[error("the update manifest is for the {saw} channel, not {expected}")]
    WrongChannel { expected: Channel, saw: Channel },
    #[error("the update manifest is stale: {0}")]
    Stale(#[from] replay::StaleError),
    #[error("the update manifest is invalid: {0}")]
    Invalid(String),
    #[error("the download failed: {0}")]
    Download(String),
    #[error("the download is {saw} bytes, the manifest says {expected}")]
    SizeMismatch { expected: u64, saw: u64 },
    #[error("the downloaded file is not the one the manifest describes")]
    DigestMismatch,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl UpdateError {
    /// The stable code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "update.not_configured",
            Self::NotPublished => "update.not_published",
            Self::Fetch(_) => "update.fetch_failed",
            Self::TooLarge => "update.too_large",
            Self::Malformed(_) => "update.malformed",
            Self::Untrusted(_) => "update.untrusted",
            Self::BadSignature(_) => "update.bad_signature",
            Self::Revoked => "update.revoked",
            Self::SchemaVersion { .. } => "update.schema_version_unsupported",
            Self::WrongChannel { .. } => "update.wrong_channel",
            Self::Stale(_) => "update.stale",
            Self::Invalid(_) => "update.invalid",
            Self::Download(_) => "update.download_failed",
            Self::SizeMismatch { .. } => "update.size_mismatch",
            Self::DigestMismatch => "update.digest_mismatch",
            Self::Other(_) => "update.failed",
        }
    }

    /// Whether this says something about the manifest's integrity rather than about reaching
    /// it: a signature, schema, channel, freshness or content refusal. Those are what a check
    /// reports even when another channel's manifest verified.
    #[must_use]
    pub fn is_integrity(&self) -> bool {
        matches!(
            self,
            Self::TooLarge
                | Self::Malformed(_)
                | Self::Untrusted(_)
                | Self::BadSignature(_)
                | Self::Revoked
                | Self::SchemaVersion { .. }
                | Self::WrongChannel { .. }
                | Self::Stale(_)
                | Self::Invalid(_)
        )
    }
}

impl UpdateManifest {
    /// The freshness fields, as [`rd_sign::replay::check`] wants them.
    #[must_use]
    pub fn freshness(&self) -> replay::Freshness {
        replay::Freshness {
            sequence: self.sequence,
            issued_at: self.issued_at,
            not_after: Some(self.not_after),
        }
    }

    /// Whether installing this version changes the database schema; `true` when the manifest
    /// does not say.
    #[must_use]
    pub fn changes_schema(&self) -> bool {
        self.schema_change.unwrap_or(true)
    }

    /// The offered version, parsed.
    pub fn semver(&self) -> Result<semver::Version, UpdateError> {
        crate::offer::parse_version(&self.version)
            .ok_or_else(|| invalid(format!("{} is not a SemVer version", self.version)))
    }

    /// Refuses a manifest this build must not act on: bounds, formats, duplicates, and a
    /// version that contradicts its channel.
    pub fn validate(&self) -> Result<(), UpdateError> {
        if self.schema_version != UPDATE_MANIFEST_SCHEMA_VERSION {
            return Err(UpdateError::SchemaVersion {
                saw: u64::from(self.schema_version),
            });
        }
        if self.sequence == 0 {
            return Err(invalid("the sequence starts at 1"));
        }
        if self.not_after <= self.issued_at {
            return Err(invalid("not_after is not after issued_at"));
        }
        if self.not_after - self.issued_at > Duration::days(MAX_VALIDITY_DAYS) {
            return Err(invalid(format!(
                "valid for longer than {MAX_VALIDITY_DAYS} days"
            )));
        }
        let version = self.semver()?;
        // A beta on the stable channel is exactly what a stable installation must never be
        // offered; a plain version on the beta channel is a manifest published in the wrong
        // place, and which half was meant is not something to guess.
        match (self.channel, version.pre.is_empty()) {
            (Channel::Stable, false) => {
                return Err(invalid(format!(
                    "{} is a pre-release on the stable channel",
                    self.version
                )));
            }
            (Channel::Beta, true) => {
                return Err(invalid(format!(
                    "{} is not a pre-release but is on the beta channel",
                    self.version
                )));
            }
            _ => {}
        }
        if self.notes.chars().count() > MAX_NOTES_CHARS {
            return Err(invalid(format!(
                "the notes are longer than {MAX_NOTES_CHARS} characters"
            )));
        }
        if self.artifacts.len() > MAX_ARTIFACTS {
            return Err(invalid(format!("more than {MAX_ARTIFACTS} artifacts")));
        }
        let mut seen = BTreeSet::new();
        for artifact in &self.artifacts {
            let entry = format!("{}/{}/{}", artifact.platform, artifact.arch, artifact.kind);
            artifact
                .validate()
                .map_err(|reason| invalid(format!("{entry}: {reason}")))?;
            if !seen.insert((
                artifact.platform.as_str(),
                artifact.arch.as_str(),
                artifact.kind.as_str(),
            )) {
                return Err(invalid(format!("{entry} is listed twice")));
            }
        }
        Ok(())
    }
}

impl Artifact {
    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("platform", &self.platform),
            ("arch", &self.arch),
            ("kind", &self.kind),
        ] {
            if value.is_empty()
                || value.len() > 32
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(format!("{name} {value:?} is not a lowercase identifier"));
            }
        }
        if self.url.len() > 1024 {
            return Err("the URL is longer than 1024 bytes".to_owned());
        }
        let url = url::Url::parse(&self.url).map_err(|error| format!("URL: {error}"))?;
        if url.scheme() != "https"
            || url.host_str().is_none_or(str::is_empty)
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(format!("{} is not a plain https:// URL", self.url));
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("sha256 is not 64 lowercase hex characters".to_owned());
        }
        if self.size == 0 || self.size > MAX_ARTIFACT_BYTES {
            return Err(format!(
                "size {} is outside 1..={MAX_ARTIFACT_BYTES}",
                self.size
            ));
        }
        Ok(())
    }
}

fn invalid(reason: impl Into<String>) -> UpdateError {
    UpdateError::Invalid(reason.into())
}

/// Verifies a manifest fetched for `channel` against the compiled-in release root.
///
/// `floor` is the highest sequence this installation has already accepted on `channel`, which
/// the caller persists; `None` only on a first contact. Unlike the plugin index a manifest at
/// the floor itself is accepted: the periodic check fetches the same manifest again and again
/// until the next release, and that is the manifest already accepted, not a replay of an older
/// one. An older manifest always has a lower sequence, because the publisher's counter is the
/// signing time.
pub fn verify(
    bytes: &[u8],
    channel: Channel,
    floor: Option<u64>,
    now: DateTime<Utc>,
) -> Result<UpdateManifest, UpdateError> {
    let trust = release_trust(now)?;
    verify_with(bytes, &trust, channel, floor, now)
}

/// The trust store of the compiled-in release root, or [`UpdateError::NotConfigured`] when this
/// build carries no release key — a clear state rather than a signature error on every check.
pub fn release_trust(now: DateTime<Utc>) -> Result<TrustStore, UpdateError> {
    if rd_sign::keys_for(Role::Release, now).is_empty() {
        return Err(UpdateError::NotConfigured);
    }
    rd_sign::trust_store_for(Role::Release, now).map_err(UpdateError::Other)
}

/// [`verify`] against an explicit trust store: a test's, or the one [`release_trust`] built.
pub fn verify_with(
    bytes: &[u8],
    trust: &TrustStore,
    channel: Channel,
    floor: Option<u64>,
    now: DateTime<Utc>,
) -> Result<UpdateManifest, UpdateError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(UpdateError::TooLarge);
    }
    let document = SignedDocument::parse(bytes)
        .map_err(|error| UpdateError::Malformed(format!("{error:#}")))?;
    let payload: serde_json::Value =
        document
            .verify(UPDATE_MANIFEST_DOMAIN, trust)
            .map_err(|error| match error {
                VerifyError::UntrustedKey { .. } => UpdateError::Untrusted(error.to_string()),
                VerifyError::BadSignature { .. } => UpdateError::BadSignature(error.to_string()),
                VerifyError::Revoked => UpdateError::Revoked,
                VerifyError::Other(other) => UpdateError::Malformed(format!("{other:#}")),
            })?;
    let saw = payload
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| UpdateError::Malformed("schema_version is missing".to_owned()))?;
    if saw != u64::from(UPDATE_MANIFEST_SCHEMA_VERSION) {
        return Err(UpdateError::SchemaVersion { saw });
    }
    let manifest: UpdateManifest = serde_json::from_value(payload)
        .map_err(|error| UpdateError::Malformed(error.to_string()))?;
    if manifest.channel != channel {
        return Err(UpdateError::WrongChannel {
            expected: channel,
            saw: manifest.channel,
        });
    }
    if let Some(floor) = floor
        && manifest.sequence < floor
    {
        return Err(replay::StaleError::Replayed {
            saw: manifest.sequence,
            known: floor,
        }
        .into());
    }
    // The floor is checked above; this is expiry and the future-timestamp bound.
    replay::check(manifest.freshness(), None, now)?;
    manifest.validate()?;
    Ok(manifest)
}

/// Signs `manifest` under `key_id` and returns the document as it is published.
///
/// Refuses a manifest [`verify_with`] would refuse for its content or size, so a broken one
/// never leaves the release workflow.
pub fn sign(
    key_id: &str,
    key: &rd_sign::SigningKey,
    manifest: &UpdateManifest,
) -> anyhow::Result<Vec<u8>> {
    manifest.validate()?;
    let document = rd_sign::sign_document(UPDATE_MANIFEST_DOMAIN, key_id, key, manifest)?;
    let bytes = serde_json::to_vec_pretty(&document)?;
    anyhow::ensure!(
        bytes.len() <= MAX_MANIFEST_BYTES,
        "the signed manifest is {} bytes, more than the {MAX_MANIFEST_BYTES} a reader accepts",
        bytes.len()
    );
    Ok(bytes)
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
pub(crate) mod tests;
