//! The catalogues hold the same sentences with the same holes, and a text is said in the
//! language asked for (RD-1480-11).

use std::collections::{BTreeMap, BTreeSet};

use super::{NotificationLanguage as L, Text, Words, fill};

fn catalogue(language: L) -> BTreeMap<String, String> {
    serde_json::from_str(language.source())
        .unwrap_or_else(|error| panic!("{language:?} does not parse: {error}"))
}

fn holes(text: &str) -> BTreeSet<String> {
    text.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name.to_owned()))
        .collect()
}

/// Every language carries every sentence with the holes the English one has, so no notification
/// falls back to English or shows a `{hole}` unfilled.
#[test]
fn every_catalogue_has_every_sentence_and_its_holes() {
    let english = catalogue(L::En);
    for language in L::ALL {
        let other = catalogue(language);
        assert_eq!(
            other.keys().collect::<BTreeSet<_>>(),
            english.keys().collect::<BTreeSet<_>>(),
            "{language:?}"
        );
        for (key, text) in &english {
            assert_eq!(holes(&other[key]), holes(text), "{language:?} {key}");
        }
    }
}

/// A sentence that reads the same in two languages is named here; any other is a forgotten
/// translation.
#[test]
fn every_sentence_is_translated() {
    const SAME: &[&str] = &[
        "automation.title",
        "package_malware.body",
        "power.standby",
        "source.api",
        "source.click_and_load",
    ];
    let english = catalogue(L::En);
    for language in [L::De, L::Es, L::Fr] {
        for (key, text) in catalogue(language) {
            assert!(
                text != english[&key] || SAME.contains(&key.as_str()),
                "{language:?} {key} is the English sentence"
            );
        }
    }
}

/// `web/src/locales/languages.json` is the one list of languages: one the interface has to ship
/// is one the notifications speak, under the code the setting stores.
#[test]
fn the_languages_are_the_interface_s_required_ones() {
    let root = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let list = std::path::Path::new(&root).join("../../web/src/locales/languages.json");
    let list: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(list).expect("languages.json"))
            .expect("a list of languages");
    let required: BTreeSet<_> = list
        .into_iter()
        .filter(|(_, entry)| entry["status"] == "required")
        .map(|(code, _)| code)
        .collect();
    let spoken: BTreeSet<_> = L::ALL
        .into_iter()
        .map(|language| {
            serde_json::to_value(language)
                .expect("code")
                .as_str()
                .expect("a string")
                .to_owned()
        })
        .collect();
    assert_eq!(spoken, required);
    assert_eq!(L::default(), L::En, "English while none is chosen");
}

#[test]
fn a_text_is_said_in_the_language_asked_for() {
    let finished = Text::new("package_completed.title").with("name", "Holiday");
    assert_eq!(finished.say(L::En), "Package finished: Holiday");
    for language in L::ALL {
        assert_eq!(
            finished.say(language),
            catalogue(language)["package_completed.title"].replace("{name}", "Holiday"),
            "{language:?}"
        );
    }
}

#[test]
fn a_count_takes_the_plural_of_its_language() {
    let links = |count| Text::counted("links_added.title", count);
    assert_eq!(links(1).say(L::En), "1 link added");
    assert_eq!(links(0).say(L::En), "0 links added");
    let french = |form: &str, count: &str| {
        catalogue(L::Fr)[&format!("links_added.title_{form}")].replace("{count}", count)
    };
    assert_eq!(links(0).say(L::Fr), french("one", "0"));
    assert_eq!(links(1).say(L::Fr), french("one", "1"));
    assert_eq!(links(2).say(L::Fr), french("other", "2"));
}

#[test]
fn a_sentence_inside_another_is_said_in_the_same_language() {
    let text = Text::new("package_failed.title").with("name", Text::new("package.unnamed"));
    assert_eq!(text.say(L::En), "Package failed: A package");
    let german = catalogue(L::De);
    assert_eq!(
        text.say(L::De),
        german["package_failed.title"].replace("{name}", &german["package.unnamed"])
    );
    let list = Words::List(vec![Text::new("source.clipboard").into(), "other".into()]);
    assert_eq!(
        list.say(L::De),
        format!("{}, other", german["source.clipboard"])
    );
}

/// A name is data: braces in it are no hole, and a sentence the catalogues lack shows its key.
#[test]
fn a_name_is_never_read_as_a_hole() {
    let text = Text::new("package_completed.title")
        .with("name", "{title} {name}")
        .with("title", "x");
    assert_eq!(text.say(L::En), "Package finished: {title} {name}");
    assert_eq!(
        fill("{a} {b} {", |name| (name == "a").then(|| "1".to_owned())),
        "1 {b} {"
    );
    assert_eq!(Text::new("no.such.sentence").say(L::De), "no.such.sentence");
}
