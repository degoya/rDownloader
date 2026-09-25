//! Tests for [`super`]: the documented example parses, every refusal has a case, and the
//! two behaviours a rule carries — claiming an address and reviving a dead host — hold.

use super::*;

/// The rule `docs/site-rules.md` walks through, kept in step with the documentation.
pub(crate) fn example_json() -> serde_json::Value {
    serde_json::json!({
        "id": "scnlog",
        "name": "scnlog.me",
        "group": "board",
        "version": 1,
        "match": {
            "hosts": ["scnlog.me", "*.scnlog.me"],
            "paths": ["^/[a-z0-9-]+/[a-z0-9-]+/?$"]
        },
        "dead": ["scnlog.eu", "scnlog.life"],
        "steps": [
            { "kind": "fetch" },
            { "kind": "regex", "pattern": "<div class=\"links\">(.+?)</div>", "into": "container" },
            { "kind": "regex", "from": "container", "pattern": "href=\"(https?://[^\"]+)\"", "into": "links", "all": true }
        ],
        "package": { "from": "regex", "pattern": "<h1[^>]*>(.+?)</h1>" },
        "probe": "https://scnlog.me/movies/some-release-2026/",
        "checked": "2026-09-20"
    })
}

pub(crate) fn example() -> Rule {
    serde_json::from_value(example_json()).expect("example rule")
}

fn url(text: &str) -> Url {
    Url::parse(text).expect("url")
}

#[test]
fn the_documented_example_is_valid_and_round_trips() {
    let rule = example();
    rule.validate().expect("valid");
    let json = serde_json::to_value(&rule).expect("encode");
    assert_eq!(json, example_json());
}

#[test]
fn an_unknown_field_is_refused_rather_than_ignored() {
    let mut json = example_json();
    json["comment"] = serde_json::Value::String("x".to_owned());
    assert!(serde_json::from_value::<Rule>(json).is_err());
    let mut json = example_json();
    json["match"]["ports"] = serde_json::json!([443]);
    assert!(serde_json::from_value::<Rule>(json).is_err());
    let mut json = example_json();
    json["package"]["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Rule>(json).is_err());
}

#[test]
fn a_rule_claims_its_hosts_and_paths() {
    let rule = example();
    assert!(rule.claims(&url("https://scnlog.me/movies/some-release/")));
    assert!(rule.claims(&url("https://www.scnlog.me/movies/some-release")));
    assert!(!rule.claims(&url("https://scnlog.me/")));
    assert!(!rule.claims(&url("https://scnlog.eu/movies/some-release/")));
    assert!(!rule.claims(&url("https://example.org/movies/some-release/")));
}

#[test]
fn a_match_without_paths_claims_every_path_and_sees_the_query() {
    let mut rule = example();
    rule.matches.paths.clear();
    assert!(rule.claims(&url("https://scnlog.me/")));
    rule.matches.paths = vec!["^/index\\.php\\?id=\\d+$".to_owned()];
    assert!(rule.claims(&url("https://scnlog.me/index.php?id=12")));
    assert!(!rule.claims(&url("https://scnlog.me/index.php")));
}

#[test]
fn a_dead_host_is_revived_onto_the_canonical_host() {
    let rule = example();
    let revived = rule.revive(&url("https://scnlog.eu/movies/some-release/?x=1"));
    assert_eq!(
        revived.map(String::from),
        Some("https://scnlog.me/movies/some-release/?x=1".to_owned())
    );
    assert!(rule.revive(&url("https://scnlog.me/movies/x/")).is_none());
    assert!(rule.revive(&url("https://other.example/")).is_none());
}

fn refused(edit: impl FnOnce(&mut serde_json::Value)) -> RuleError {
    let mut json = example_json();
    edit(&mut json);
    let rule: Rule = serde_json::from_value(json).expect("shape");
    rule.validate().expect_err("refused")
}

#[test]
fn identity_fields_are_checked() {
    assert!(matches!(
        refused(|j| j["id"] = "Scn Log".into()),
        RuleError::Id(_)
    ));
    assert!(matches!(
        refused(|j| j["name"] = "  ".into()),
        RuleError::Name
    ));
    assert!(matches!(
        refused(|j| j["group"] = "Boards".into()),
        RuleError::Group(_)
    ));
    assert!(matches!(
        refused(|j| j["version"] = 0.into()),
        RuleError::Version
    ));
}

#[test]
fn hosts_are_checked() {
    assert!(matches!(
        refused(|j| j["match"]["hosts"] = serde_json::json!([])),
        RuleError::NoHosts
    ));
    assert!(matches!(
        refused(|j| j["match"]["hosts"] = serde_json::json!(["*.scnlog.me"])),
        RuleError::WildcardCanonical
    ));
    assert!(matches!(
        refused(|j| j["match"]["hosts"] = serde_json::json!(["scnlog.me", "https://x.me"])),
        RuleError::Host(_)
    ));
    assert!(matches!(
        refused(|j| j["dead"] = serde_json::json!(["scnlog.me"])),
        RuleError::DeadIsLive(_)
    ));
    assert!(matches!(
        refused(|j| j["dead"] = serde_json::json!(["www.scnlog.me"])),
        RuleError::DeadIsLive(_)
    ));
    assert!(matches!(
        refused(|j| j["dead"] = serde_json::json!(["localhost"])),
        RuleError::Host(_)
    ));
}

#[test]
fn patterns_steps_and_package_are_checked() {
    assert!(matches!(
        refused(|j| j["match"]["paths"] = serde_json::json!(["("])),
        RuleError::Pattern { .. }
    ));
    assert!(matches!(
        refused(|j| j["steps"] = serde_json::json!([])),
        RuleError::NoSteps
    ));
    assert!(matches!(
        refused(|j| j["steps"][1]["pattern"] = "[".into()),
        RuleError::Pattern { .. }
    ));
    assert!(matches!(
        refused(|j| j["package"] = serde_json::json!({"from": "regex", "pattern": "("})),
        RuleError::Pattern { .. }
    ));
    assert!(matches!(
        refused(|j| j["package"] = serde_json::json!({"from": "variable", "name": "Title"})),
        RuleError::Variable(_)
    ));
}

#[test]
fn the_probe_must_be_an_address_the_rule_claims() {
    assert!(matches!(
        refused(|j| j["probe"] = "scnlog.me/movies/x/".into()),
        RuleError::Probe(_)
    ));
    assert!(matches!(
        refused(|j| j["probe"] = "ftp://scnlog.me/movies/x/".into()),
        RuleError::Probe(_)
    ));
    assert!(matches!(
        refused(|j| j["probe"] = "https://scnlog.eu/movies/x/".into()),
        RuleError::ProbeUnclaimed(_)
    ));
    assert!(matches!(
        refused(|j| j["probe"] = "https://scnlog.me/".into()),
        RuleError::ProbeUnclaimed(_)
    ));
}

#[test]
fn checked_is_a_calendar_date() {
    let mut json = example_json();
    json["checked"] = "20.09.2026".into();
    assert!(serde_json::from_value::<Rule>(json).is_err());
}
