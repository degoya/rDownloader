//! Which external tool versions this build works with, and what a bad one costs (RD-102-03).
//!
//! The shape of the answer matters more than the rules themselves. Four verdicts, and they
//! are deliberately not collapsible into a boolean:
//!
//! * [`Verdict::Supported`] — the version is at or above the floor and not on a bad list.
//! * [`Verdict::TooOld`] — below the floor. Actionable: there is a version to upgrade to.
//! * [`Verdict::KnownBad`] — a specific release that is broken for a specific job. Upgrading
//!   is not the only answer; downgrading may be.
//! * [`Verdict::Unknown`] — the version could not be read, or nothing has an opinion about
//!   this tool. **Never blocks.** "We could not tell" is not evidence of a fault, and treating
//!   it as one would turn every unusual build into a broken installation.
//!
//! **A rule names the capabilities it affects, and only those are gated.** A yt-dlp below the
//! floor stops media downloads; it does not stop the Usenet queue, HTTP transfers or anything
//! else. There is no path here that refuses work in general.
//!
//! **The compiled-in base is the floor and the fallback.** Rules can also arrive inside the
//! signed tool manifest, which is verified against the compiled-in root and the replay rule
//! before it is ever seen here. Any failure at all — a bad signature, a stale document, a rule
//! naming a tool this build does not look up, a version string that does not parse — drops the
//! whole delivered set and leaves [`CompatRules::base`] in force. A rule set that cannot be
//! read is not a rule set, and inventing a partial one from it would make the shipped floor
//! depend on the shape of the damage.
//!
//! **An override is explicit and leaves a record.** A tool named in the settings still gets a
//! verdict and still shows a warning; what it loses is the block, and every evaluation that
//! skipped a block emits a `tracing` record naming the tool, the verdict and the rule.

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, LazyLock, RwLock},
};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    manifest::ToolManifest,
    version::{self, DetectedVersion, ToolVersion},
};

/// The tools a compatibility rule may name.
///
/// Closed, like [`crate::manifest::MANAGED_TOOLS`], and for the same reason: a rule about a
/// name nothing ever looks up cannot gate anything, so accepting one would only make the
/// delivered set look larger than it is. `unrar`, `7z` and `rclone` are absent because no
/// capability here is derived from their version; their failures are reported by the tools
/// themselves. `par2` is absent because repair runs in-process, with no binary at all.
pub const RULED_TOOLS: &[&str] = &["yt-dlp", "gallery-dl", "streamlink", "ffmpeg", "ffprobe"];

/// The largest delivered rule set this build reads, so a signed but oversized document is
/// refused rather than walked.
const MAX_RULES: usize = 64;

/// What a tool version can take away.
///
/// One variant per gate that actually exists in the code, so a rule cannot promise a block
/// that nothing enforces.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Probing and downloading media through yt-dlp.
    MediaDownload,
    /// Merging a separate video and audio stream, which needs ffmpeg *and* ffprobe.
    MediaMerge,
    /// Extracting and re-encoding audio, i.e. the MP3 variants.
    AudioExtraction,
    /// Downloading an image gallery through gallery-dl.
    GalleryDownload,
    /// Recording a live stream through Streamlink.
    StreamRecording,
}

impl Capability {
    /// The stable identifier used as a REST parameter and as a translation key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MediaDownload => "media_download",
            Self::MediaMerge => "media_merge",
            Self::AudioExtraction => "audio_extraction",
            Self::GalleryDownload => "gallery_download",
            Self::StreamRecording => "stream_recording",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a rule says about the version that was found.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// At or above the floor, and not listed as broken.
    Supported,
    /// Below the floor this build is tested against.
    TooOld,
    /// A specific release listed as broken for the capabilities the rule names.
    KnownBad,
    /// No version could be read, or no rule has an opinion. Never blocks.
    #[default]
    Unknown,
}

impl Verdict {
    /// The stable identifier used in REST responses and as a translation key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::TooOld => "too_old",
            Self::KnownBad => "known_bad",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this verdict is a fault rather than an absence of information.
    #[must_use]
    pub const fn is_incompatible(self) -> bool {
        matches!(self, Self::TooOld | Self::KnownBad)
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One tool's compatibility policy.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CompatRule {
    /// One of [`RULED_TOOLS`].
    pub tool: String,
    /// The oldest version this build is tested against. `None` sets no floor.
    #[serde(default)]
    pub min_version: Option<String>,
    /// Individual releases that are broken regardless of the floor.
    #[serde(default)]
    pub known_bad: Vec<String>,
    /// What is lost when the verdict is not [`Verdict::Supported`]. Must name at least one
    /// capability: a rule that gates nothing cannot say what a warning is about.
    #[serde(default)]
    pub affects: Vec<Capability>,
}

/// Why a delivered rule set was refused.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// A rule about a tool this build never looks up.
    #[error("compatibility rule names {0}, which is not a tool this build gates on")]
    UnknownTool(String),
    /// A rule that gates nothing.
    #[error("the compatibility rule for {tool} names no capability")]
    NoCapability { tool: String },
    /// A version string in the rule that cannot be compared against anything.
    #[error("the compatibility rule for {tool} carries an unreadable version {version:?}")]
    UnreadableVersion { tool: String, version: String },
    /// More rules than this build reads.
    #[error("the compatibility rule set carries {0} rules, more than this build reads")]
    TooMany(usize),
}

/// A tool, the version found and what the rules make of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assessment {
    /// The tool this is about.
    pub tool: String,
    /// The verdict; see [`Verdict`].
    pub verdict: Verdict,
    /// The version that was read, normalised when it parsed and raw when it did not.
    pub version: Option<String>,
    /// The floor the rule sets, when it sets one.
    pub min_version: Option<String>,
    /// The capabilities the rule covers; empty when no rule applies.
    pub affects: Vec<Capability>,
    /// Whether the settings name this tool as overridden, so no block is enforced.
    pub overridden: bool,
}

impl Assessment {
    /// An assessment for a tool that was not found at all.
    ///
    /// [`Verdict::Unknown`], because a missing tool is reported as missing by the lookup
    /// itself; saying it is also incompatible would be two names for one fact.
    #[must_use]
    pub fn unknown(tool: &str) -> Self {
        Self {
            tool: tool.to_owned(),
            verdict: Verdict::Unknown,
            version: None,
            min_version: None,
            affects: Vec::new(),
            overridden: false,
        }
    }

    /// Whether this assessment forbids `capability`.
    ///
    /// False for [`Verdict::Unknown`] under every circumstance, and false whenever the tool
    /// is overridden.
    #[must_use]
    pub fn blocks(&self, capability: Capability) -> bool {
        !self.overridden && self.verdict.is_incompatible() && self.affects.contains(&capability)
    }

    /// Whether a warning is worth showing, i.e. the version is or might be a problem.
    #[must_use]
    pub fn warns(&self) -> bool {
        self.verdict.is_incompatible()
            || (self.verdict == Verdict::Unknown && !self.affects.is_empty())
    }

    /// What to do about it, in one English sentence, or `None` when there is nothing to do.
    ///
    /// English on purpose: this is `doctor` output and a log line, not interface text. The
    /// interface builds its own sentence from [`Self::verdict`] and [`Self::affects`], which
    /// is why both are carried separately.
    #[must_use]
    pub fn upgrade(&self) -> Option<String> {
        match self.verdict {
            Verdict::TooOld => Some(match &self.min_version {
                Some(minimum) => format!("update {} to {minimum} or newer", self.tool),
                None => format!("update {}", self.tool),
            }),
            Verdict::KnownBad => Some(format!(
                "{} {} is listed as broken; install a different version",
                self.tool,
                self.version.as_deref().unwrap_or("this version")
            )),
            Verdict::Supported | Verdict::Unknown => None,
        }
    }

    /// A one-line summary for `doctor`.
    #[must_use]
    pub fn summary(&self) -> String {
        let capabilities: Vec<&str> = self.affects.iter().map(|value| value.as_str()).collect();
        let mut text = match self.verdict {
            Verdict::Supported => "supported".to_owned(),
            Verdict::Unknown => "unknown version".to_owned(),
            Verdict::TooOld => match &self.min_version {
                Some(minimum) => format!("too old, needs {minimum} or newer"),
                None => "too old".to_owned(),
            },
            Verdict::KnownBad => "known bad".to_owned(),
        };
        if self.verdict != Verdict::Supported && !capabilities.is_empty() {
            text.push_str(&format!(" (affects {})", capabilities.join(", ")));
        }
        if self.overridden && self.verdict.is_incompatible() {
            text.push_str(" [override in force]");
        }
        text
    }
}

/// The rules in force, as a set that can be replaced whole.
#[derive(Clone, Debug)]
pub struct CompatRules {
    rules: Vec<CompatRule>,
}

impl CompatRules {
    /// The rules compiled into this build.
    ///
    /// Each floor is the oldest version this project tests against, not a guess at where a
    /// tool broke. `known_bad` is empty in every shipped rule: withdrawing a specific release
    /// is a statement about the world after this build was made, and it belongs in the signed
    /// manifest rather than frozen into a binary.
    #[must_use]
    pub fn base() -> Self {
        Self {
            rules: vec![
                CompatRule {
                    tool: "yt-dlp".to_owned(),
                    min_version: Some("2023.01.06".to_owned()),
                    known_bad: Vec::new(),
                    affects: vec![Capability::MediaDownload],
                },
                CompatRule {
                    tool: "gallery-dl".to_owned(),
                    min_version: Some("1.25.0".to_owned()),
                    known_bad: Vec::new(),
                    affects: vec![Capability::GalleryDownload],
                },
                CompatRule {
                    tool: "streamlink".to_owned(),
                    min_version: Some("5.5.0".to_owned()),
                    known_bad: Vec::new(),
                    affects: vec![Capability::StreamRecording],
                },
                CompatRule {
                    tool: "ffmpeg".to_owned(),
                    min_version: Some("4.4".to_owned()),
                    known_bad: Vec::new(),
                    affects: vec![Capability::MediaMerge, Capability::AudioExtraction],
                },
                CompatRule {
                    tool: "ffprobe".to_owned(),
                    min_version: Some("4.4".to_owned()),
                    known_bad: Vec::new(),
                    affects: vec![Capability::MediaMerge, Capability::AudioExtraction],
                },
            ],
        }
    }

    /// Validates `delivered` and lays it over [`Self::base`], rule by tool.
    ///
    /// A tool the delivered set does not mention keeps its compiled-in rule, so a manifest
    /// that only has something to say about yt-dlp cannot silently drop the FFmpeg floor.
    ///
    /// # Errors
    ///
    /// [`RuleError`] when any single rule is unusable. The whole set is refused rather than
    /// the offending rule dropped: a document this build cannot read completely is one it
    /// should not act on partially.
    pub fn layered_over_base(delivered: Vec<CompatRule>) -> Result<Self, RuleError> {
        if delivered.len() > MAX_RULES {
            return Err(RuleError::TooMany(delivered.len()));
        }
        for rule in &delivered {
            validate(rule)?;
        }
        let mut rules = Self::base().rules;
        for rule in delivered {
            match rules.iter_mut().find(|existing| existing.tool == rule.tool) {
                Some(existing) => *existing = rule,
                None => rules.push(rule),
            }
        }
        Ok(Self { rules })
    }

    /// The rules a verified manifest delivers, laid over the compiled-in base.
    ///
    /// # Errors
    ///
    /// [`RuleError`] when the manifest's rules cannot be read; see
    /// [`Self::layered_over_base`].
    pub fn from_manifest(manifest: &ToolManifest) -> Result<Self, RuleError> {
        Self::layered_over_base(manifest.compatibility.clone())
    }

    /// The rule for `tool`, when there is one.
    #[must_use]
    pub fn rule(&self, tool: &str) -> Option<&CompatRule> {
        self.rules.iter().find(|rule| rule.tool == tool)
    }

    /// Judges a detected version against the rule for `tool`.
    #[must_use]
    pub fn assess(&self, tool: &str, detected: &DetectedVersion, overridden: bool) -> Assessment {
        let Some(rule) = self.rule(tool) else {
            return Assessment {
                version: detected.display(),
                overridden,
                ..Assessment::unknown(tool)
            };
        };
        let verdict = match &detected.parsed {
            // Nothing to compare: the rule exists, the version does not. Unknown, and the
            // affected capabilities are still carried so the interface can say what it could
            // not verify.
            None => Verdict::Unknown,
            Some(found) => {
                if rule
                    .known_bad
                    .iter()
                    .filter_map(|text| ToolVersion::parse(text))
                    .any(|bad| bad == *found)
                {
                    Verdict::KnownBad
                } else if rule
                    .min_version
                    .as_deref()
                    .and_then(ToolVersion::parse)
                    .is_some_and(|minimum| *found < minimum)
                {
                    Verdict::TooOld
                } else {
                    Verdict::Supported
                }
            }
        };
        Assessment {
            tool: tool.to_owned(),
            verdict,
            version: detected.display(),
            min_version: rule.min_version.clone(),
            affects: rule.affects.clone(),
            overridden,
        }
    }
}

/// Refuses a rule this build must not act on.
fn validate(rule: &CompatRule) -> Result<(), RuleError> {
    if !RULED_TOOLS.contains(&rule.tool.as_str()) {
        return Err(RuleError::UnknownTool(rule.tool.clone()));
    }
    if rule.affects.is_empty() {
        return Err(RuleError::NoCapability {
            tool: rule.tool.clone(),
        });
    }
    for version in rule.min_version.iter().chain(rule.known_bad.iter()) {
        if ToolVersion::parse(version).is_none() {
            return Err(RuleError::UnreadableVersion {
                tool: rule.tool.clone(),
                version: version.clone(),
            });
        }
    }
    Ok(())
}

/// The rules and overrides in force for this process.
#[derive(Clone, Debug)]
struct Active {
    rules: CompatRules,
    overrides: BTreeSet<String>,
}

static ACTIVE: LazyLock<RwLock<Arc<Active>>> = LazyLock::new(|| {
    RwLock::new(Arc::new(Active {
        rules: CompatRules::base(),
        overrides: BTreeSet::new(),
    }))
});

/// The rules and overrides currently in force. Base rules and no overrides until something
/// says otherwise, which is what makes a process that never starts the tool service behave
/// exactly like one that did and found nothing.
fn active() -> Arc<Active> {
    ACTIVE
        .read()
        .map(|active| Arc::clone(&active))
        .unwrap_or_else(|_| {
            Arc::new(Active {
                rules: CompatRules::base(),
                overrides: BTreeSet::new(),
            })
        })
}

fn replace(build: impl FnOnce(&Active) -> Active) {
    if let Ok(mut current) = ACTIVE.write() {
        *current = Arc::new(build(&current));
    }
}

/// Puts a rule set in force.
pub fn set_rules(rules: CompatRules) {
    replace(|current| Active {
        rules,
        overrides: current.overrides.clone(),
    });
}

/// Puts the explicit overrides in force, as tool names.
///
/// Unknown names are dropped rather than refused: the setting is a list of tools, and a tool
/// this build does not gate on simply has no rule to override.
pub fn set_overrides(tools: &[String]) {
    let overrides: BTreeSet<String> = tools
        .iter()
        .map(|tool| tool.trim().to_lowercase())
        .filter(|tool| RULED_TOOLS.contains(&tool.as_str()))
        .collect();
    replace(|current| Active {
        rules: current.rules.clone(),
        overrides,
    });
}

/// Adopts the rules a verified manifest carries, degrading to [`CompatRules::base`] on any
/// failure.
///
/// The manifest reaching here has already passed signature, schema and replay verification;
/// this is the second half of the same rule — a document that verified but whose rules this
/// build cannot read leaves the compiled-in floor in force, and says so.
pub fn adopt_manifest(manifest: &ToolManifest) {
    match CompatRules::from_manifest(manifest) {
        Ok(rules) => set_rules(rules),
        Err(error) => {
            tracing::warn!(
                %error,
                "the tool manifest's compatibility rules are unusable; \
                 the compiled-in base rules stay in force"
            );
            set_rules(CompatRules::base());
        }
    }
}

/// The rules in force.
#[must_use]
pub fn rules() -> CompatRules {
    active().rules.clone()
}

/// Judges an already-detected version against the rules in force.
#[must_use]
pub fn assess_detected(tool: &str, detected: &DetectedVersion) -> Assessment {
    let active = active();
    let assessment = active
        .rules
        .assess(tool, detected, active.overrides.contains(tool));
    audit(&assessment);
    assessment
}

/// Reads the version at `path` — from the cache when the binary has not changed — and judges
/// it against the rules in force.
pub async fn assess(tool: &str, path: &Path) -> Assessment {
    let detected = version::detect(tool, path).await;
    assess_detected(tool, &detected)
}

/// Records an override that actually suppressed a block.
///
/// This is the auditable half of "override only explicit and auditable": the setting is the
/// explicit half, and nothing skips a block without leaving this line behind.
fn audit(assessment: &Assessment) {
    if assessment.overridden && assessment.verdict.is_incompatible() {
        tracing::warn!(
            tool = %assessment.tool,
            verdict = %assessment.verdict,
            version = assessment.version.as_deref().unwrap_or("unknown"),
            min_version = assessment.min_version.as_deref().unwrap_or("none"),
            affects = %assessment
                .affects
                .iter()
                .map(|capability| capability.as_str())
                .collect::<Vec<_>>()
                .join(","),
            "a tool compatibility rule is overridden by an explicit setting; \
             the affected capabilities are not blocked"
        );
    }
}

/// The failure a blocked capability raises.
///
/// One code beside `media.tool_missing`, carrying the capability as a parameter, because the
/// interface has to be able to say *what* stopped working — "yt-dlp is incompatible" leaves a
/// reader to guess whether their Usenet queue is affected too.
#[must_use]
pub fn incompatible_failure(assessment: &Assessment, capability: Capability) -> rd_core::Failure {
    let version = assessment.version.as_deref().unwrap_or("unknown");
    let mut failure = rd_core::Failure::coded(
        rd_core::FailureKind::Unsupported,
        "media.tool_incompatible",
        match assessment.upgrade() {
            Some(upgrade) => format!(
                "{} {version} is not compatible with {capability}: {upgrade}",
                assessment.tool
            ),
            None => format!(
                "{} {version} is not compatible with {capability}",
                assessment.tool
            ),
        },
    )
    .with_param("tool", &assessment.tool)
    .with_param("version", version)
    .with_param("capability", capability.as_str());
    if let Some(minimum) = &assessment.min_version {
        failure = failure.with_param("min_version", minimum);
    }
    failure
}

#[cfg(test)]
mod tests {
    use super::{Capability, CompatRule, CompatRules, RuleError, Verdict};
    use crate::version::{DetectedVersion, ToolVersion};

    fn detected(text: &str) -> DetectedVersion {
        DetectedVersion {
            raw: Some(text.to_owned()),
            parsed: ToolVersion::parse(text),
        }
    }

    fn rule(tool: &str, min: Option<&str>, bad: &[&str]) -> CompatRule {
        CompatRule {
            tool: tool.to_owned(),
            min_version: min.map(str::to_owned),
            known_bad: bad.iter().map(|value| (*value).to_owned()).collect(),
            affects: vec![Capability::MediaDownload],
        }
    }

    /// The whole point of the four states: each one is reachable and they are distinct.
    #[test]
    fn the_four_verdicts_are_distinguishable() {
        let rules = CompatRules::layered_over_base(vec![rule(
            "yt-dlp",
            Some("2024.01.01"),
            &["2024.05.05"],
        )])
        .expect("rules");
        assert_eq!(
            rules
                .assess("yt-dlp", &detected("2024.08.06"), false)
                .verdict,
            Verdict::Supported
        );
        assert_eq!(
            rules
                .assess("yt-dlp", &detected("2023.12.31"), false)
                .verdict,
            Verdict::TooOld
        );
        assert_eq!(
            rules
                .assess("yt-dlp", &detected("2024.05.05"), false)
                .verdict,
            Verdict::KnownBad
        );
        assert_eq!(
            rules
                .assess("yt-dlp", &detected("N-1-gabcdef"), false)
                .verdict,
            Verdict::Unknown
        );
    }

    /// A verdict is useless without the capability it is about.
    #[test]
    fn an_incompatible_verdict_names_the_affected_capability() {
        let rules = CompatRules::base();
        let assessment = rules.assess("ffmpeg", &detected("4.2.7"), false);
        assert_eq!(assessment.verdict, Verdict::TooOld);
        assert_eq!(
            assessment.affects,
            vec![Capability::MediaMerge, Capability::AudioExtraction]
        );
        let failure = super::incompatible_failure(&assessment, Capability::MediaMerge);
        assert_eq!(failure.code.as_deref(), Some("media.tool_incompatible"));
        assert_eq!(
            failure.params.get("capability").map(String::as_str),
            Some("media_merge")
        );
    }

    /// Only the capabilities the rule names are gated; a rule about yt-dlp says nothing about
    /// gallery downloads, and nothing here can refuse work in general.
    #[test]
    fn only_the_named_capabilities_are_blocked() {
        let rules = CompatRules::base();
        let assessment = rules.assess("yt-dlp", &detected("2020.01.01"), false);
        assert!(assessment.blocks(Capability::MediaDownload));
        assert!(!assessment.blocks(Capability::GalleryDownload));
        assert!(!assessment.blocks(Capability::MediaMerge));
    }

    /// Unknown is not a fault. An unreadable version warns and blocks nothing.
    #[test]
    fn an_unreadable_version_never_blocks() {
        let rules = CompatRules::base();
        let assessment = rules.assess("ffmpeg", &detected("N-113522-g8b0a3d5c"), false);
        assert_eq!(assessment.verdict, Verdict::Unknown);
        assert!(assessment.warns());
        for capability in [Capability::MediaMerge, Capability::AudioExtraction] {
            assert!(!assessment.blocks(capability));
        }
    }

    /// A tool nothing has an opinion about is Unknown with no capabilities, so it warns about
    /// nothing either.
    #[test]
    fn a_tool_without_a_rule_neither_warns_nor_blocks() {
        let assessment = CompatRules::base().assess("unrar", &detected("6.24"), false);
        assert_eq!(assessment.verdict, Verdict::Unknown);
        assert!(assessment.affects.is_empty());
        assert!(!assessment.warns());
    }

    /// The override keeps the verdict and drops the block; that is what makes it auditable
    /// rather than a way of making the problem disappear.
    #[test]
    fn an_override_keeps_the_verdict_and_drops_the_block() {
        let assessment = CompatRules::base().assess("yt-dlp", &detected("2020.01.01"), true);
        assert_eq!(assessment.verdict, Verdict::TooOld);
        assert!(assessment.warns());
        assert!(!assessment.blocks(Capability::MediaDownload));
        assert!(assessment.summary().contains("override in force"));
    }

    /// A delivered set replaces the rules for the tools it names and leaves the rest of the
    /// compiled-in floor standing.
    #[test]
    fn a_delivered_rule_layers_over_the_base_without_dropping_it() {
        let rules = CompatRules::layered_over_base(vec![rule("yt-dlp", Some("2025.01.01"), &[])])
            .expect("rules");
        assert_eq!(
            rules
                .rule("yt-dlp")
                .and_then(|rule| rule.min_version.clone()),
            Some("2025.01.01".to_owned())
        );
        assert_eq!(
            rules
                .rule("ffmpeg")
                .and_then(|rule| rule.min_version.clone()),
            Some("4.4".to_owned())
        );
    }

    /// Every way a delivered rule can be unusable refuses the whole set, so the caller falls
    /// back to the compiled-in base rather than to a half-read one.
    #[test]
    fn an_unusable_rule_refuses_the_whole_delivered_set() {
        assert!(matches!(
            CompatRules::layered_over_base(vec![rule("curl", Some("8.0"), &[])]),
            Err(RuleError::UnknownTool(name)) if name == "curl"
        ));
        assert!(matches!(
            CompatRules::layered_over_base(vec![rule("yt-dlp", Some("whenever"), &[])]),
            Err(RuleError::UnreadableVersion { .. })
        ));
        assert!(matches!(
            CompatRules::layered_over_base(vec![rule("yt-dlp", None, &["not-a-version"])]),
            Err(RuleError::UnreadableVersion { .. })
        ));
        let mut gateless = rule("yt-dlp", Some("2024.01.01"), &[]);
        gateless.affects.clear();
        assert!(matches!(
            CompatRules::layered_over_base(vec![gateless]),
            Err(RuleError::NoCapability { .. })
        ));
        let many = std::iter::repeat_with(|| rule("yt-dlp", Some("2024.01.01"), &[]))
            .take(super::MAX_RULES + 1)
            .collect();
        assert!(matches!(
            CompatRules::layered_over_base(many),
            Err(RuleError::TooMany(_))
        ));
    }

    /// A distribution's build of the minimum version is that version, not one below it.
    #[test]
    fn a_packaging_suffix_still_meets_the_floor() {
        let assessment = CompatRules::base().assess("ffmpeg", &detected("4.4-6ubuntu5"), false);
        assert_eq!(assessment.verdict, Verdict::Supported);
    }
}
