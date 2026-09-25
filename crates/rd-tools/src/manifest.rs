//! The signed document that says which tool builds exist, where they live and what they hash to.
//!
//! Everything this crate installs is named by a manifest first. That is the whole security
//! model: the URL, the SHA-256 and the size all come from a document signed by the
//! compiled-in tool-manifest root, so a download is checked against a statement that was made
//! before the download happened. A URL typed into the settings would move the decision to
//! whoever answers that URL, which is exactly what a signature is for.
//!
//! Two properties are worth naming.
//!
//! **A signature says who, never when.** A manifest carries a monotonic `sequence` and an
//! `issued_at`/`not_after` pair, checked through [`rd_sign::replay`] against the highest
//! sequence this installation has already accepted. Without that, an attacker who can answer
//! the refresh request replays last month's genuine, genuinely signed manifest forever and the
//! installation never learns that a newer tool build exists.
//!
//! **The build shipped with the application is the floor.** [`embedded`] is verified with the
//! same root and the same code path as a fetched one; a refresh only ever moves forward from
//! it. An installation with no network, or one whose manifest URL is unreachable, therefore
//! still has a manifest — it simply has an old one, and says so.

use chrono::{DateTime, Utc};
use rd_sign::{Role, SignedDocument, VerifyError, replay};
use serde::{Deserialize, Serialize};

use crate::error::ToolError;

/// Domain separator for the tool manifest's signatures.
///
/// Bound into the digest so a signature over a tool manifest cannot be lifted onto an update
/// manifest or a repository index that happens to parse under the same key.
pub const TOOL_MANIFEST_DOMAIN: &str = "rdownloader.tool-manifest.v1";

/// The manifest layout this build understands. A document declaring anything else is refused
/// rather than read optimistically.
pub const TOOL_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// The tools this application will ever manage.
///
/// Deliberately closed. Managing an arbitrary name would mean the manifest could make the
/// application download and run anything at all under a name nothing here ever looks up, and
/// a signed document is not a reason to widen that.
///
/// `par2` is absent on purpose: repair runs in-process through `rust_par2`, so there is no
/// binary to manage. `unrar`, `7z` and `rclone` are absent because their upstreams publish no
/// per-platform release feed this can pin against; they stay vendor/`PATH` tools.
pub const MANAGED_TOOLS: &[&str] = &["yt-dlp", "gallery-dl", "streamlink", "ffmpeg", "ffprobe"];

/// Whether `name` is one of the tools this application manages.
#[must_use]
pub fn is_managed_tool(name: &str) -> bool {
    MANAGED_TOOLS.contains(&name)
}

/// The signed manifest payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolManifest {
    /// Always [`TOOL_MANIFEST_SCHEMA_VERSION`] for a document this build accepts.
    pub schema_version: u32,
    /// The publisher's monotonic counter. Never goes backwards; see [`rd_sign::replay`].
    pub sequence: u64,
    /// When the publisher says it signed this.
    pub issued_at: DateTime<Utc>,
    /// After this instant the manifest is stale even if nothing newer has been seen.
    #[serde(default)]
    pub not_after: Option<DateTime<Utc>>,
    /// One entry per tool, version and platform.
    #[serde(default)]
    pub tools: Vec<ToolEntry>,
    /// Version-range and known-bad policy per tool (RD-102-03).
    ///
    /// Carried in this document rather than in one of its own, so a rule is covered by the
    /// same signature, the same domain separator and the same replay floor as the builds it
    /// talks about. Absent in a manifest published before the field existed, which is why it
    /// defaults: the payload is signed as the bytes it arrived as, so adding a field here
    /// leaves every already-signed manifest verifying, with the compiled-in base rules in
    /// force. See [`crate::compat`] for what happens when a delivered rule cannot be read.
    #[serde(default)]
    pub compatibility: Vec<crate::compat::CompatRule>,
}

impl ToolManifest {
    /// The freshness fields, as [`rd_sign::replay::check`] wants them.
    #[must_use]
    pub fn freshness(&self) -> replay::Freshness {
        replay::Freshness {
            sequence: self.sequence,
            issued_at: self.issued_at,
            not_after: self.not_after,
        }
    }

    /// Entries for `name` on `platform` that this application version may run, newest first.
    #[must_use]
    pub fn releases(&self, name: &str, platform: &str, app_version: &str) -> Vec<&ToolEntry> {
        let mut matching: Vec<&ToolEntry> = self
            .tools
            .iter()
            .filter(|entry| entry.name == name && entry.platform == platform)
            .filter(|entry| entry.suits_application(app_version))
            .collect();
        matching.sort_by(|left, right| compare_versions(&right.version, &left.version));
        matching
    }

    /// The newest runnable release of `name`, or `None` when the manifest offers none.
    #[must_use]
    pub fn newest_release(
        &self,
        name: &str,
        platform: &str,
        app_version: &str,
    ) -> Option<&ToolEntry> {
        self.releases(name, platform, app_version)
            .into_iter()
            .next()
    }

    /// The entry for one exact version, if the manifest carries it for this platform.
    #[must_use]
    pub fn release(
        &self,
        name: &str,
        version: &str,
        platform: &str,
        app_version: &str,
    ) -> Option<&ToolEntry> {
        self.releases(name, platform, app_version)
            .into_iter()
            .find(|entry| entry.version == version)
    }
}

/// One downloadable tool build.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolEntry {
    /// One of [`MANAGED_TOOLS`].
    pub name: String,
    /// The tool's own version string, used verbatim as a directory name after validation.
    pub version: String,
    /// Rust target triple, e.g. `x86_64-unknown-linux-gnu`, matched against
    /// [`crate::platform::current`].
    pub platform: String,
    /// Where the bytes are. Must be `https://`.
    pub url: String,
    /// Lowercase hex SHA-256 of the bytes at `url`, checked before anything is activated.
    pub sha256: String,
    /// Expected byte count, so a download that grows without bound is cut off early rather
    /// than after it has filled the disk.
    pub size: u64,
    /// How the payload is packed.
    #[serde(default)]
    pub archive: ArchiveFormat,
    /// For an archive format, the members to extract, each landing under its own file name in
    /// the version directory. Empty means "every regular member".
    ///
    /// A member may be named by its full path inside the archive or by its base name; either
    /// way only the base name survives into the store, so a nested archive layout is
    /// flattened. An archive whose members would collide once flattened cannot be described
    /// here — see `docs/external-tools.md` for the one tool that fails on exactly that.
    #[serde(default)]
    pub members: Vec<String>,
    /// Lowest application version this build is declared to work with.
    #[serde(default)]
    pub min_app_version: Option<String>,
    /// First application version this build is *not* declared to work with, exclusive.
    #[serde(default)]
    pub max_app_version: Option<String>,
}

impl ToolEntry {
    /// Whether the compatibility window covers `app_version`.
    ///
    /// An unparseable bound refuses the entry. A window that cannot be read is not a window,
    /// and "install it anyway" is the wrong way to resolve a broken manifest.
    #[must_use]
    pub fn suits_application(&self, app_version: &str) -> bool {
        let Ok(application) = semver::Version::parse(app_version) else {
            // The application's own version is generated from Cargo metadata, so this is a
            // build fault rather than a manifest fault; refuse rather than guess.
            return false;
        };
        let within_lower = match &self.min_app_version {
            Some(text) => semver::Version::parse(text).is_ok_and(|bound| application >= bound),
            None => true,
        };
        let within_upper = match &self.max_app_version {
            Some(text) => semver::Version::parse(text).is_ok_and(|bound| application < bound),
            None => true,
        };
        within_lower && within_upper
    }
}

/// How a manifest entry's bytes are packed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFormat {
    /// The bytes are the executable itself.
    #[default]
    Raw,
    /// A ZIP archive; [`ToolEntry::members`] says what to take out of it.
    Zip,
    /// An xz-compressed tar archive, which is how the Linux FFmpeg builds are published.
    /// [`ToolEntry::members`] says what to take out of it, exactly as for [`Self::Zip`].
    TarXz,
}

/// Orders two tool versions, preferring a semver reading and falling back to a string compare.
///
/// Tool version strings are not all semver — yt-dlp publishes `2024.08.06` — so a parse
/// failure has to keep working rather than collapse every version to "equal".
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    match (semver::Version::parse(left), semver::Version::parse(right)) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// The manifest compiled into this build, as signed bytes.
///
/// Shipped rather than fetched so the feature has a floor that no network can move.
pub const EMBEDDED_MANIFEST_BYTES: &[u8] = include_bytes!("../resources/tools-manifest.json");

/// The compiled-in manifest, verified through the same path a fetched one takes.
///
/// Verified rather than trusted: it is checked against the same root, so a build whose
/// resource was swapped between compile and release fails here instead of installing
/// something. Freshness is checked with no known sequence, because this document *is* the
/// starting point.
pub fn embedded(now: DateTime<Utc>) -> Result<ToolManifest, ToolError> {
    verify(EMBEDDED_MANIFEST_BYTES, None, now)
}

/// Verifies a signed manifest against the compiled-in tool-manifest root and the freshness
/// rule, returning the payload only when both hold.
///
/// `known_sequence` is the highest sequence this installation has already accepted, which the
/// caller has to persist — a replay check that forgets is not one.
pub fn verify(
    bytes: &[u8],
    known_sequence: Option<u64>,
    now: DateTime<Utc>,
) -> Result<ToolManifest, ToolError> {
    let trust = rd_sign::trust_store_for(Role::ToolManifest, now)
        .map_err(|error| ToolError::ManifestUntrusted(error.to_string()))?;
    verify_with(bytes, &trust, known_sequence, now)
}

/// [`verify`] against an explicit trust store, so a test can prove that another key is
/// refused without editing the shipped roots.
pub fn verify_with(
    bytes: &[u8],
    trust: &rd_sign::TrustStore,
    known_sequence: Option<u64>,
    now: DateTime<Utc>,
) -> Result<ToolManifest, ToolError> {
    let document = SignedDocument::parse(bytes)
        .map_err(|error| ToolError::ManifestUntrusted(error.to_string()))?;
    let manifest: ToolManifest = document
        .verify(TOOL_MANIFEST_DOMAIN, trust)
        .map_err(|error| match error {
            VerifyError::UntrustedKey { .. }
            | VerifyError::BadSignature { .. }
            | VerifyError::Revoked => ToolError::ManifestUntrusted(error.to_string()),
            VerifyError::Other(other) => ToolError::Other(other),
        })?;
    if manifest.schema_version != TOOL_MANIFEST_SCHEMA_VERSION {
        return Err(ToolError::ManifestUntrusted(format!(
            "tool manifest declares schema version {}, this build reads {TOOL_MANIFEST_SCHEMA_VERSION}",
            manifest.schema_version
        )));
    }
    replay::check(manifest.freshness(), known_sequence, now)?;
    for entry in &manifest.tools {
        entry.validate()?;
    }
    Ok(manifest)
}

impl ToolEntry {
    /// Refuses an entry this build must not act on, before anything is downloaded.
    fn validate(&self) -> Result<(), ToolError> {
        if !is_managed_tool(&self.name) {
            return Err(ToolError::NotManaged(self.name.clone()));
        }
        crate::store::validate_segment(&self.version)?;
        if !self.url.starts_with("https://") {
            return Err(ToolError::ManifestUntrusted(format!(
                "{} {} is published over a non-https URL",
                self.name, self.version
            )));
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ToolError::ManifestUntrusted(format!(
                "{} {} carries no usable SHA-256",
                self.name, self.version
            )));
        }
        if self.size == 0 || self.size > crate::download::MAX_TOOL_BYTES {
            return Err(ToolError::ManifestUntrusted(format!(
                "{} {} declares an implausible size of {} bytes",
                self.name, self.version, self.size
            )));
        }
        Ok(())
    }
}

/// Wraps `manifest` in a signed document, for whoever publishes one.
///
/// Lives here rather than in a build script so the publishing path and the verifying path
/// share one definition of the domain string and the payload shape.
pub fn sign(
    key_id: &str,
    key: &rd_sign::SigningKey,
    manifest: &ToolManifest,
) -> anyhow::Result<Vec<u8>> {
    let document = rd_sign::sign_document(TOOL_MANIFEST_DOMAIN, key_id, key, manifest)?;
    Ok(serde_json::to_vec_pretty(&document)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, min: Option<&str>, max: Option<&str>) -> ToolEntry {
        ToolEntry {
            name: "yt-dlp".to_owned(),
            version: version.to_owned(),
            platform: "x86_64-unknown-linux-gnu".to_owned(),
            url: "https://example.invalid/yt-dlp".to_owned(),
            sha256: "0".repeat(64),
            size: 1024,
            archive: ArchiveFormat::Raw,
            members: Vec::new(),
            min_app_version: min.map(str::to_owned),
            max_app_version: max.map(str::to_owned),
        }
    }

    fn manifest(tools: Vec<ToolEntry>) -> ToolManifest {
        ToolManifest {
            schema_version: TOOL_MANIFEST_SCHEMA_VERSION,
            sequence: 1,
            issued_at: Utc::now(),
            not_after: None,
            tools,
            compatibility: Vec::new(),
        }
    }

    /// The whole point of the compatibility window: a build outside it is not offered.
    #[test]
    fn a_release_outside_the_compatibility_window_is_not_offered() {
        let manifest = manifest(vec![
            entry("2024.01.01", None, Some("1.0.0")),
            entry("2024.06.01", Some("1.0.0"), None),
        ]);
        let offered = manifest.releases("yt-dlp", "x86_64-unknown-linux-gnu", "1.0.1");
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].version, "2024.06.01");
    }

    /// An unreadable bound refuses the entry rather than installing it anyway.
    #[test]
    fn an_unparseable_compatibility_bound_refuses_the_entry() {
        assert!(!entry("2024.01.01", Some("whenever"), None).suits_application("1.0.1"));
    }

    /// Newest first, and a non-semver tool version still orders sensibly.
    #[test]
    fn releases_are_offered_newest_first() {
        let manifest = manifest(vec![
            entry("2024.01.01", None, None),
            entry("2024.09.07", None, None),
            entry("2024.06.01", None, None),
        ]);
        let offered = manifest.releases("yt-dlp", "x86_64-unknown-linux-gnu", "1.0.1");
        let versions: Vec<&str> = offered.iter().map(|entry| entry.version.as_str()).collect();
        assert_eq!(versions, ["2024.09.07", "2024.06.01", "2024.01.01"]);
    }

    /// A name outside the closed list must not reach the download path at all.
    #[test]
    fn an_unmanaged_tool_name_is_refused_by_validation() {
        let mut foreign = entry("1.0.0", None, None);
        foreign.name = "curl".to_owned();
        assert!(matches!(
            foreign.validate(),
            Err(ToolError::NotManaged(name)) if name == "curl"
        ));
    }

    /// http:// in a signed document is still http://; the signature does not make it safe to
    /// take bytes over it.
    #[test]
    fn a_plain_http_url_is_refused() {
        let mut insecure = entry("1.0.0", None, None);
        insecure.url = "http://example.invalid/yt-dlp".to_owned();
        assert!(matches!(
            insecure.validate(),
            Err(ToolError::ManifestUntrusted(_))
        ));
    }

    /// The build's own manifest has to verify against the build's own root, or the feature
    /// ships broken.
    #[test]
    fn the_embedded_manifest_verifies_against_the_compiled_in_root() {
        let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
        assert_eq!(manifest.schema_version, TOOL_MANIFEST_SCHEMA_VERSION);
    }

    /// Every shipped entry has to pass the validation an install applies. A hash of the wrong
    /// length or a name outside [`MANAGED_TOOLS`] would otherwise only surface at the moment
    /// somebody tries to install it.
    #[test]
    fn every_entry_of_the_embedded_manifest_validates() {
        let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
        assert!(
            !manifest.tools.is_empty(),
            "the shipped manifest has to carry the builds it promises"
        );
        for entry in &manifest.tools {
            if let Err(error) = entry.validate() {
                panic!(
                    "{} {} on {} is unusable: {error}",
                    entry.name, entry.version, entry.platform
                );
            }
        }
    }

    /// What the shipped manifest actually covers, written down as a test so the support
    /// matrix in `docs/external-tools.md` cannot drift away from the document.
    #[test]
    fn the_embedded_manifest_covers_the_platforms_the_documentation_claims() {
        let manifest = embedded(Utc::now()).expect("embedded manifest verifies");
        let application = env!("CARGO_PKG_VERSION");
        for platform in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"] {
            for tool in ["yt-dlp", "ffmpeg", "ffprobe"] {
                assert!(
                    manifest
                        .newest_release(tool, platform, application)
                        .is_some(),
                    "{tool} on {platform}"
                );
            }
        }
        for platform in ["aarch64-unknown-linux-gnu", "aarch64-pc-windows-msvc"] {
            assert!(
                manifest
                    .newest_release("yt-dlp", platform, application)
                    .is_some(),
                "yt-dlp on {platform}"
            );
        }
        // gallery-dl and streamlink are managed tools with no manageable release; see
        // `docs/external-tools.md` for why. They resolve through the vendor folders instead.
        for tool in ["gallery-dl", "streamlink"] {
            assert!(
                manifest
                    .newest_release(tool, "x86_64-unknown-linux-gnu", application)
                    .is_none(),
                "{tool}"
            );
        }
    }
}
