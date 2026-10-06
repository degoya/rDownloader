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
    let rules =
        CompatRules::layered_over_base(vec![rule("yt-dlp", Some("2024.01.01"), &["2024.05.05"])])
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
    let assessment = CompatRules::base().assess("rclone", &detected("1.66.0"), false);
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

/// Review 2026-09-28, finding 5: unrar below 6.12 and 7-Zip below 25.00 write through an
/// archive's links, p7zip 16.02 included. The floor sits exactly at the fixed releases.
#[test]
fn the_archive_tools_have_a_security_floor() {
    let rules = CompatRules::base();
    for (tool, version, verdict) in [
        ("unrar", "6.11", Verdict::TooOld),
        ("unrar", "6.12", Verdict::Supported),
        ("unrar", "7.01", Verdict::Supported),
        ("7z", "16.02", Verdict::TooOld),
        ("7z", "24.09", Verdict::TooOld),
        ("7z", "25.00", Verdict::Supported),
        ("7z", "25.01", Verdict::Supported),
    ] {
        let assessment = rules.assess(tool, &detected(version), false);
        assert_eq!(assessment.verdict, verdict, "{tool} {version}");
        assert_eq!(assessment.affects, vec![Capability::ArchiveExtraction]);
    }
}

/// The floor is compiled in: a delivered rule cannot lower it, and an override cannot
/// lift it.
#[test]
fn neither_a_delivered_rule_nor_an_override_reaches_the_archive_floor() {
    for tool in ["unrar", "7z"] {
        assert!(matches!(
            CompatRules::layered_over_base(vec![rule(tool, Some("5.0"), &[])]),
            Err(RuleError::UnknownTool(name)) if name == tool
        ));
    }
    super::set_overrides(&["unrar".to_owned(), "7z".to_owned()]);
    let assessment = super::assess_detected("unrar", &detected("6.11"));
    super::set_overrides(&[]);
    assert!(
        !assessment.overridden,
        "an override named the archive floor"
    );
    assert!(assessment.blocks(Capability::ArchiveExtraction));
}
