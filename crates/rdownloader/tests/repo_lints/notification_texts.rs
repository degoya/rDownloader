//! The catalogues of the notifications' built-in wording (`crates/rd-notify/locales`) against the
//! code that says their sentences (RD-1480-11): a key the code names exists, a sentence nobody
//! says is gone, and the reader is addressed as everywhere else -- "du", "vous", "tu".
//!
//! That the four catalogues hold the same sentences with the same holes is `rd-notify`'s own test.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::french_spanish_address::{addresses_as_tu, addresses_as_usted};
use crate::german_address::addresses_formally;
use crate::workspace_root;

/// The crates that say a notification's sentences.
const SOURCES: &[&str] = &[
    "crates/rd-notify/src",
    "crates/rd-api-core/src",
    "crates/rd-api-admin/src",
];

fn catalogue(language: &str) -> BTreeMap<String, String> {
    let path = workspace_root().join(format!("crates/rd-notify/locales/{language}.json"));
    let text = std::fs::read_to_string(&path).expect("catalogue");
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn sources(directory: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read dir") {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            sources(&path, out);
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
            out.push(path);
        }
    }
}

/// The key behind every `call(` in `text`: the string literal, blanks before it skipped.
fn keys_of(text: &str, call: &str) -> Vec<String> {
    text.split(call)
        .skip(1)
        .filter_map(|rest| rest.trim_start().strip_prefix('"'))
        .filter_map(|rest| rest.split_once('"'))
        .map(|(key, _)| key.to_owned())
        .collect()
}

#[test]
fn every_sentence_the_code_says_exists_and_none_is_left_over() {
    let mut files = Vec::new();
    for directory in SOURCES {
        sources(&workspace_root().join(directory), &mut files);
    }
    let mut said = BTreeSet::new();
    // A key chosen first and said afterwards (an intake's source) is a literal somewhere else.
    let mut literals = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("source");
        said.extend(keys_of(&text, "Text::new("));
        for key in keys_of(&text, "Text::counted(") {
            said.extend([format!("{key}_one"), format!("{key}_other")]);
        }
        literals.extend(text.split('"').map(str::to_owned));
    }
    assert!(said.len() > 80, "expected to read the notifications' texts");
    let known: BTreeSet<_> = catalogue("en").into_keys().collect();
    let missing: Vec<_> = said.difference(&known).collect();
    assert!(missing.is_empty(), "no such sentence: {missing:?}");
    let unsaid: Vec<_> = known
        .difference(&said)
        .filter(|key| !literals.contains(*key))
        .collect();
    assert!(unsaid.is_empty(), "nobody says: {unsaid:?}");
}

#[test]
fn the_reader_is_addressed_as_everywhere_else() {
    for (language, offends) in [
        ("de", addresses_formally as fn(&str) -> bool),
        ("fr", addresses_as_tu),
        ("es", addresses_as_usted),
    ] {
        let offenders: Vec<_> = catalogue(language)
            .into_iter()
            .filter(|(_, text)| offends(text))
            .collect();
        assert!(offenders.is_empty(), "{language}: {offenders:?}");
    }
}
