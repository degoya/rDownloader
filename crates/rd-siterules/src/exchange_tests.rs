use super::*;

fn document() -> Exchange {
    serde_json::from_str(EXAMPLES).expect("the example file reads as an exchange file")
}

/// Every entry of the example file is a complete, valid rule, so [`examples`] leaves none out.
#[test]
fn every_example_reads_and_validates() {
    let document = document();
    assert_eq!(document.format_version, EXCHANGE_VERSION);
    assert!(
        (3..=6).contains(&document.rules.len()),
        "three to six examples, not {}",
        document.rules.len()
    );
    for entry in &document.rules {
        let rule: Rule = serde_json::from_value(entry.rule.clone())
            .unwrap_or_else(|error| panic!("{}: {error}", entry.rule["id"]));
        rule.validate()
            .unwrap_or_else(|error| panic!("{} is invalid: {error}", rule.id));
    }
    assert_eq!(examples().len(), document.rules.len());
}

/// Switched off, one group, a description each, and ids that do not repeat (RD-1230-03).
#[test]
fn the_examples_arrive_switched_off_and_explain_themselves() {
    let document = document();
    assert!(document.rules.iter().all(|entry| !entry.enabled));
    let rules = examples();
    let mut ids: Vec<&str> = rules.iter().map(|rule| rule.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), rules.len(), "an id repeats");
    for rule in &rules {
        assert_eq!(rule.group, "examples", "{}", rule.id);
        let description = rule.description.as_deref().unwrap_or_default();
        assert!(description.len() > 80, "{} explains too little", rule.id);
    }
}

/// At least one example shows the two-stage choice (`groups.pick`), and at least one is the
/// plain one-stage shape.
#[test]
fn the_examples_show_both_shapes() {
    let rules = examples();
    assert!(rules.iter().any(|rule| {
        rule.groups
            .as_ref()
            .is_some_and(|groups| groups.pick.is_some())
    }));
    assert!(rules.iter().any(|rule| rule.groups.is_none()));
}

/// The file round-trips: what the export writes for an example is what the file holds.
#[test]
fn an_example_writes_back_what_was_read() {
    for entry in document().rules {
        let rule: Rule = serde_json::from_value(entry.rule.clone()).expect("rule");
        assert_eq!(
            serde_json::to_value(&rule).expect("encode"),
            entry.rule,
            "{}",
            rule.id
        );
    }
}

/// A file of the old layout, the bodies without their switches, is not an exchange file.
#[test]
fn the_old_layout_does_not_read_as_an_exchange_file() {
    let old = serde_json::json!({ "format_version": 1, "rules": [{ "id": "x" }] });
    assert!(serde_json::from_value::<Exchange>(old).is_err());
}
