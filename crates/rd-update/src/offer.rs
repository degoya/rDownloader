//! Which verified release, if any, this installation is offered.
//!
//! Three rules, each one a thing an update check must never do:
//!
//! * **No downgrade.** Only a version of higher SemVer precedence than the running one is
//!   offered; going back is a deliberate recovery, never a notice (RD-180-01).
//! * **No beta on the stable channel.** A stable installation reads only stable manifests, and
//!   a pre-release version is refused even if one of them carried it.
//! * **The beta channel sees stable releases too**, so an installation on `1.8.0-beta.2` is
//!   offered `1.8.0` once it is out, and keeps receiving the stable line from then on.

use chrono::{DateTime, Utc};
use semver::{Prerelease, Version};
use serde::{Deserialize, Serialize};

use crate::{
    InstallKind,
    manifest::{Artifact, Channel, UpdateManifest, kind},
};

/// What the interface shows about an available update.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Offer {
    pub version: String,
    pub channel: Channel,
    pub released_at: DateTime<Utc>,
    pub notes: String,
    /// The anchor of the version's `CHANGELOG.md` heading, when the manifest names it.
    #[serde(default)]
    pub changelog_anchor: Option<String>,
    /// The file for this platform, architecture and installation kind, when the release has one.
    pub artifact: Option<Artifact>,
    /// Whether installing it changes the database schema ([`UpdateManifest::changes_schema`]);
    /// an offer stored before the manifest said so reads as `true`.
    #[serde(default = "changes_schema_by_default")]
    pub schema_change: bool,
}

const fn changes_schema_by_default() -> bool {
    true
}

/// The artifact an installation would take: its platform, architecture and kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    pub platform: &'static str,
    pub arch: &'static str,
    pub kind: &'static str,
}

impl Target {
    /// The running build's platform and architecture, with the artifact kind `install` takes.
    #[must_use]
    pub fn current(install: InstallKind) -> Self {
        Self {
            platform: match std::env::consts::OS {
                "macos" => "macos",
                "windows" => "windows",
                _ => "linux",
            },
            arch: std::env::consts::ARCH,
            kind: install.artifact_kind(),
        }
    }
}

/// `text` as SemVer, with or without the tag's leading `v`.
#[must_use]
pub fn parse_version(text: &str) -> Option<Version> {
    Version::parse(text.trim().trim_start_matches('v')).ok()
}

/// SemVer precedence: build metadata does not count, a release outranks its pre-releases.
fn precedence(version: &Version) -> (u64, u64, u64, Prerelease) {
    (
        version.major,
        version.minor,
        version.patch,
        version.pre.clone(),
    )
}

/// Whether `candidate` is a newer version than `current`. Anything unparseable is not newer.
#[must_use]
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => precedence(&candidate) > precedence(&current),
        _ => false,
    }
}

/// The newest release among `manifests` that `channel` may offer over `current`, with the
/// artifact for `target`; `None` when this installation is up to date.
///
/// `manifests` are verified already; this only chooses.
#[must_use]
pub fn newest_offer(
    current: &str,
    channel: Channel,
    manifests: &[UpdateManifest],
    target: &Target,
) -> Option<Offer> {
    newest_in(current, channel, manifests, target, application_artifacts)
}

/// [`newest_offer`] for the capture agent installed without the service (RD-1210-03): the same
/// rules, the artifact from the release's own agent archives. A release without them is still
/// offered, without an artifact — the agent then names the download page.
#[must_use]
pub fn newest_agent_offer(
    current: &str,
    channel: Channel,
    manifests: &[UpdateManifest],
    target: &Target,
) -> Option<Offer> {
    newest_in(current, channel, manifests, target, agent_artifacts)
}

fn application_artifacts(manifest: &UpdateManifest) -> &[Artifact] {
    &manifest.artifacts
}

fn agent_artifacts(manifest: &UpdateManifest) -> &[Artifact] {
    &manifest.agent_artifacts
}

fn newest_in(
    current: &str,
    channel: Channel,
    manifests: &[UpdateManifest],
    target: &Target,
    artifacts: fn(&UpdateManifest) -> &[Artifact],
) -> Option<Offer> {
    let current = parse_version(current)?;
    manifests
        .iter()
        .filter_map(|manifest| Some((manifest, manifest.semver().ok()?)))
        .filter(|(manifest, version)| match channel {
            Channel::Stable => manifest.channel == Channel::Stable && version.pre.is_empty(),
            Channel::Beta => true,
        })
        .filter(|(_, version)| precedence(version) > precedence(&current))
        .max_by(|(_, left), (_, right)| precedence(left).cmp(&precedence(right)))
        .map(|(manifest, _)| Offer {
            version: manifest.version.clone(),
            channel: manifest.channel,
            released_at: manifest.released_at,
            notes: manifest.notes.clone(),
            changelog_anchor: manifest.changelog_anchor.clone(),
            artifact: artifacts(manifest)
                .iter()
                .find(|artifact| {
                    artifact.platform == target.platform
                        && artifact.arch == target.arch
                        && artifact.kind == target.kind
                })
                .cloned(),
            schema_change: manifest.changes_schema(),
        })
}

impl Offer {
    /// The version's `CHANGELOG.md` section at its tag in the official repository: the full
    /// changes, which stay put while `main` moves on (RD-1150-02). Without an anchor, the file at
    /// the tag.
    #[must_use]
    pub fn changelog_url(&self) -> String {
        let file = format!(
            "https://github.com/{}/blob/v{}/CHANGELOG.md",
            crate::check::OFFICIAL_REPOSITORY,
            self.version
        );
        match &self.changelog_anchor {
            Some(anchor) => format!("{file}#{anchor}"),
            None => file,
        }
    }
}

impl InstallKind {
    /// The artifact kind this installation would be updated from.
    #[must_use]
    pub fn artifact_kind(self) -> &'static str {
        match self {
            Self::Msi => kind::MSI,
            Self::Deb => kind::DEB,
            Self::Rpm => kind::RPM,
            _ => kind::ARCHIVE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::tests::{artifact, manifest};

    const LINUX: Target = Target {
        platform: "linux",
        arch: "x86_64",
        kind: "archive",
    };

    #[test]
    fn a_newer_stable_release_is_offered_with_this_platforms_artifact() {
        let offer = newest_offer(
            "1.7.0",
            Channel::Stable,
            &[manifest(Channel::Stable, "1.8.0", 1)],
            &LINUX,
        )
        .expect("offer");
        assert_eq!(offer.version, "1.8.0");
        assert_eq!(offer.artifact, Some(artifact("linux", "x86_64", "archive")));
        // The manifest does not say: the offer counts as a schema change.
        assert!(offer.schema_change);
    }

    #[test]
    fn the_full_changes_link_to_the_changelog_section_at_the_tag() {
        let mut anchored = manifest(Channel::Stable, "1.15.0", 1);
        anchored.changelog_anchor = Some("1150---2026-10-10".to_owned());
        let offer = newest_offer("1.14.0", Channel::Stable, &[anchored], &LINUX).expect("offer");
        assert_eq!(
            offer.changelog_url(),
            "https://github.com/degoya/rDownloader/blob/v1.15.0/CHANGELOG.md#1150---2026-10-10"
        );
        let plain = newest_offer(
            "1.14.0",
            Channel::Stable,
            &[manifest(Channel::Stable, "1.15.0", 1)],
            &LINUX,
        )
        .expect("offer");
        assert_eq!(
            plain.changelog_url(),
            "https://github.com/degoya/rDownloader/blob/v1.15.0/CHANGELOG.md"
        );
    }

    #[test]
    fn the_offer_carries_what_the_manifest_says_about_the_schema() {
        let mut quiet = manifest(Channel::Stable, "1.8.0", 1);
        quiet.schema_change = Some(false);
        let offer = newest_offer("1.7.0", Channel::Stable, &[quiet], &LINUX).expect("offer");
        assert!(!offer.schema_change);
        // An offer stored before the field existed reads as a schema change.
        let mut stored = serde_json::to_value(&offer).expect("value");
        stored
            .as_object_mut()
            .expect("object")
            .remove("schema_change");
        let read: Offer = serde_json::from_value(stored).expect("read");
        assert!(read.schema_change);
    }

    /// No downgrade, and not the version already running.
    #[test]
    fn the_same_or_an_older_version_is_never_offered() {
        for current in ["1.8.0", "1.9.0", "1.8.1-beta.1"] {
            assert_eq!(
                newest_offer(
                    current,
                    Channel::Stable,
                    &[manifest(Channel::Stable, "1.8.0", 1)],
                    &LINUX
                ),
                None,
                "{current}"
            );
        }
    }

    #[test]
    fn the_stable_channel_never_offers_a_beta() {
        let manifests = [
            manifest(Channel::Stable, "1.7.1", 1),
            manifest(Channel::Beta, "1.8.0-beta.2", 2),
        ];
        let offer = newest_offer("1.7.0", Channel::Stable, &manifests, &LINUX).expect("offer");
        assert_eq!(offer.version, "1.7.1");
    }

    #[test]
    fn the_beta_channel_offers_the_newest_of_both_lines() {
        let manifests = [
            manifest(Channel::Stable, "1.7.1", 1),
            manifest(Channel::Beta, "1.8.0-beta.2", 2),
        ];
        let offer = newest_offer("1.7.0", Channel::Beta, &manifests, &LINUX).expect("offer");
        assert_eq!(offer.version, "1.8.0-beta.2");
        assert_eq!(offer.channel, Channel::Beta);
        // Once the release is out, a beta installation is offered it over its own beta.
        let manifests = [
            manifest(Channel::Stable, "1.8.0", 3),
            manifest(Channel::Beta, "1.8.0-beta.2", 2),
        ];
        let offer = newest_offer("1.8.0-beta.2", Channel::Beta, &manifests, &LINUX).expect("offer");
        assert_eq!(offer.version, "1.8.0");
    }

    /// Back on stable, a beta installation waits for the release rather than going back.
    #[test]
    fn leaving_the_beta_channel_does_not_downgrade() {
        let manifests = [manifest(Channel::Stable, "1.7.1", 1)];
        assert_eq!(
            newest_offer("1.8.0-beta.2", Channel::Stable, &manifests, &LINUX),
            None
        );
    }

    #[test]
    fn a_platform_the_release_lacks_is_offered_without_an_artifact() {
        let target = Target {
            platform: "macos",
            arch: "aarch64",
            kind: "archive",
        };
        let offer = newest_offer(
            "1.7.0",
            Channel::Stable,
            &[manifest(Channel::Stable, "1.8.0", 1)],
            &target,
        )
        .expect("offer");
        assert_eq!(offer.artifact, None);
    }

    /// RD-1210-03: the agent is offered its own archive, never the application's, and like the
    /// service never a version that is not newer than the one it runs.
    #[test]
    fn the_agent_takes_its_own_archive_and_never_an_older_version() {
        let mut release = manifest(Channel::Stable, "1.8.0", 1);
        release.agent_artifacts = vec![crate::manifest::tests::agent_artifact("linux", "x86_64")];
        let offer = newest_agent_offer("1.7.0", Channel::Stable, &[release.clone()], &LINUX)
            .expect("offer");
        assert_eq!(offer.version, "1.8.0");
        assert_eq!(
            offer.artifact.map(|artifact| artifact.url),
            Some(release.agent_artifacts[0].url.clone())
        );
        for current in ["1.8.0", "1.9.0", "2.0.0-beta.1"] {
            assert_eq!(
                newest_agent_offer(current, Channel::Stable, &[release.clone()], &LINUX),
                None,
                "{current}"
            );
        }
        // A release without agent archives is offered without one; the service's archive is
        // never taken for it.
        let offer = newest_agent_offer(
            "1.7.0",
            Channel::Stable,
            &[manifest(Channel::Stable, "1.8.0", 1)],
            &LINUX,
        )
        .expect("offer");
        assert_eq!(offer.artifact, None);
    }

    #[test]
    fn versions_compare_by_semver_precedence() {
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(is_newer("v1.8.0", "1.8.0-beta.2"));
        assert!(is_newer("1.8.0-beta.10", "1.8.0-beta.2"));
        assert!(!is_newer("1.8.0+build.5", "1.8.0"));
        assert!(!is_newer("garbage", "1.8.0"));
    }
}
