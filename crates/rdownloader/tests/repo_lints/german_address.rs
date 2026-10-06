//! The German plugin catalogues say "du", never "Sie" (owner, 2026-09-27, RD-150-16).
//!
//! A formal address is always capitalised, so any "Sie", "Ihnen" or "Ihr..." in a
//! `plugins/*/locales/de.json` is a finding. The one legitimate capital is the third person at
//! the start of a sentence ("Sie ist ein Paper-Dokument" -- the file), which no pattern can tell
//! from the reader; those are listed by plugin, key and phrase, and the phrase is cut out of that
//! one string only. `web/src/i18n/germanAddress.test.ts` and
//! `extension/test/german-address.test.mjs` hold the same rule for the other catalogues.

use crate::{strings, workspace_root};

use serde_json::Value;

const FORMAL: &[&str] = &[
    "Sie", "Ihnen", "Ihr", "Ihre", "Ihren", "Ihrem", "Ihrer", "Ihres",
];

/// Third person at a sentence start: plugin directory, key path and the phrase that carries it.
const THIRD_PERSON: &[(&str, &str, &str)] = &[
    (
        "dropbox",
        "codes.dropbox.download_not_permitted",
        "Sie ist ein Paper",
    ),
    (
        "nextcloud-crawler",
        "codes.nextcloud_crawler.share_unreachable",
        "Sie ist",
    ),
];

/// Every string of every German plugin catalogue as (plugin, key path, text).
fn german_strings() -> Vec<(String, String, String)> {
    let plugins = workspace_root().join("plugins");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&plugins).expect("plugins directory") {
        let directory = entry.expect("plugin entry").path();
        let catalogue = directory.join("locales").join("de.json");
        let Ok(text) = std::fs::read_to_string(&catalogue) else {
            continue;
        };
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{}: {error}", catalogue.display()));
        let plugin = directory
            .file_name()
            .expect("plugin directory name")
            .to_string_lossy()
            .into_owned();
        let mut found = Vec::new();
        strings(&value, "", &mut found);
        out.extend(
            found
                .into_iter()
                .map(|(key, text)| (plugin.clone(), key, text)),
        );
    }
    out
}

fn addresses_formally(text: &str) -> bool {
    text.split(|c: char| !c.is_alphabetic())
        .any(|word| FORMAL.contains(&word))
}

#[test]
fn german_plugin_texts_never_address_the_reader_as_sie() {
    let strings = german_strings();
    assert!(
        strings.len() > 200,
        "expected to read the plugin catalogues"
    );
    let offenders: Vec<String> = strings
        .iter()
        .filter(|(plugin, key, text)| {
            let rest = THIRD_PERSON
                .iter()
                .filter(|(p, k, _)| *p == plugin.as_str() && *k == key.as_str())
                .fold(text.clone(), |rest, (_, _, phrase)| {
                    rest.replacen(*phrase, "", 1)
                });
            addresses_formally(&rest)
        })
        .map(|(plugin, key, text)| format!("{plugin} {key}: {text}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "formal address in a German plugin catalogue:\n{}",
        offenders.join("\n")
    );
}

/// A rewritten sentence leaves its entry behind; the list stays as short as the catalogues need.
#[test]
fn third_person_list_names_only_phrases_that_are_still_there() {
    let strings = german_strings();
    let stale: Vec<String> = THIRD_PERSON
        .iter()
        .filter(|(plugin, key, phrase)| {
            !strings.iter().any(|(p, k, text)| {
                p.as_str() == *plugin && k.as_str() == *key && text.contains(*phrase)
            })
        })
        .map(|(plugin, key, phrase)| format!("{plugin} {key}: {phrase}"))
        .collect();
    assert!(stale.is_empty(), "stale entries:\n{}", stale.join("\n"));
}
