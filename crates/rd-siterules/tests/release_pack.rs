//! The signed rule file every release carries verifies against the compiled-in root.
//!
//! Since RD-130-07 the file is a release artifact rather than part of the binary, and the
//! check that `serve` used to run at start is the one the import runs when somebody hands the
//! file over. Here it runs without a service, so a resource that was edited without being
//! re-signed fails the build's tests rather than the person's import.

use chrono::Utc;

const RELEASE_PACK: &[u8] = include_bytes!("../resources/site-rules.json");

#[test]
fn the_release_file_verifies_against_the_compiled_in_root() {
    let pack =
        rd_siterules::verify(RELEASE_PACK, None, Utc::now()).expect("the release file verifies");
    assert_eq!(pack.format_version, rd_siterules::FORMAT_VERSION);
    let mut ids: Vec<&str> = pack.rules.iter().map(|rule| rule.id.as_str()).collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count, "the file's ids are unique");
}

/// Every rule in the file can be self-tested (RD-110-09): it names a real address, that
/// address is one the rule itself claims, and it says when the service was last measured. A
/// rule that cannot be probed is a rule nobody is watching, so this is a release gate rather
/// than a nicety.
#[test]
fn every_rule_in_the_file_carries_a_probe_its_own_match_claims_and_a_check_date() {
    let pack =
        rd_siterules::verify(RELEASE_PACK, None, Utc::now()).expect("the release file verifies");
    assert!(
        !pack.rules.is_empty(),
        "a file with no rules proves nothing"
    );
    let today = Utc::now().date_naive();
    for rule in &pack.rules {
        rule.validate()
            .unwrap_or_else(|error| panic!("rule {} is invalid: {error}", rule.id));
        let probe = url::Url::parse(&rule.probe)
            .unwrap_or_else(|error| panic!("probe of {} is not an address: {error}", rule.id));
        assert!(
            rule.claims(&probe),
            "the probe of {} is not claimed by its own match",
            rule.id
        );
        assert!(
            rule.checked <= today,
            "rule {} claims to have been measured in the future",
            rule.id
        );
    }
}
