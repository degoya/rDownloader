//! Every line a person reads about the bundle is a code, translated in all four catalogues,
//! and the archive stays readable for somebody who has no application in front of them
//! (RD-120-15).
//!
//! The English sentences themselves appear nowhere in this file on purpose: they are read out
//! of `web/src/locales/en/logs.json`, which is the one place they live.

use std::{io::Read, path::PathBuf};

use chrono::{TimeZone, Utc};
use rd_diagnostics::{BundleInput, Check, CheckStatus, Note, bundle, notes};

fn input() -> BundleInput {
    BundleInput {
        application_version: "1.2.0".to_owned(),
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        plugins: vec![("ddownload".to_owned(), "0.3.1".to_owned())],
        configuration: serde_json::json!({
            "max_active_files": 3,
            "admin_password": "hunter2",
            "nested": { "api_key": "AKIAIOSFODNN7EXAMPLE" }
        }),
        checks: vec![Check::new(
            "tools",
            "yt-dlp",
            CheckStatus::Ok,
            "/usr/bin/yt-dlp",
        )],
        recent_errors: Vec::new(),
        log_records_total: 0,
    }
}

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn catalogue(language: &str) -> serde_json::Value {
    let path = repository()
        .join("web/src/locales")
        .join(language)
        .join("logs.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
    serde_json::from_str(&text).expect("catalogue is JSON")
}

fn lookup<'a>(catalogue: &'a serde_json::Value, code: &str) -> Option<&'a str> {
    let mut node = catalogue;
    for segment in code.split('.') {
        node = node.get(segment)?;
    }
    node.as_str()
}

/// Every note the inventory can show, wherever it sits.
fn all_notes(inventory: &rd_diagnostics::Inventory) -> Vec<Note> {
    let mut all = inventory.excluded.clone();
    for entry in &inventory.entries {
        all.push(entry.description.clone());
        all.extend(entry.redactions.iter().cloned());
    }
    all
}

#[test]
fn every_code_is_translated_in_all_four_languages() {
    for language in ["de", "en", "es", "fr"] {
        let catalogue = catalogue(language);
        for code in notes::ALL_CODES {
            let text =
                lookup(&catalogue, code).unwrap_or_else(|| panic!("{language} has no logs.{code}"));
            assert!(!text.trim().is_empty(), "{language}: logs.{code} is empty");
        }
    }
}

#[test]
fn the_parameterised_code_keeps_its_placeholder_in_every_language() {
    for language in ["de", "en", "es", "fr"] {
        let catalogue = catalogue(language);
        let text = lookup(&catalogue, notes::REDACTION_CONFIGURATION_REPLACED).expect("code");
        assert!(
            text.contains(&format!("{{{}}}", notes::FIELDS_PARAMETER)),
            "{language} drops the field list from the replaced note"
        );
    }
}

#[test]
fn no_note_is_a_sentence_written_in_this_crate() {
    let inventory = bundle::inventory(&input()).expect("inventory");
    for note in all_notes(&inventory) {
        assert!(
            notes::ALL_CODES.contains(&note.code.as_str()),
            "{} is not a declared code",
            note.code
        );
        assert_eq!(
            note.text,
            notes::english(&note.code, &note.params),
            "{} carries a text that is not the catalogue's",
            note.code
        );
        assert_ne!(
            note.text, note.code,
            "{} has no English rendering in the catalogue",
            note.code
        );
    }
}

/// The guard that keeps the prose from coming back: no sentence of the catalogue may appear in
/// a source file of this crate again, whatever else somebody writes there.
#[test]
fn no_catalogue_sentence_lives_in_a_source_file_of_this_crate() {
    let english = catalogue("en");
    let mut sentences = Vec::new();
    collect(&english["diagnostics"], &mut sentences);
    assert!(english["diagnostics"].is_object(), "found the group");
    // Every code but the parameterised one, whose prose is too short to match unambiguously.
    assert!(
        sentences.len() >= notes::ALL_CODES.len() - 1,
        "collected the sentences"
    );

    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    for directory in ["src", "tests"] {
        for entry in std::fs::read_dir(crate_root.join(directory)).expect("read crate") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    assert!(sources.len() > 3, "found the sources");

    for path in sources {
        let text = std::fs::read_to_string(&path).expect("read source");
        for sentence in &sentences {
            assert!(
                !text.contains(sentence.as_str()),
                "{} writes a user-facing sentence that belongs in the catalogue: {sentence}",
                path.display()
            );
        }
    }
}

/// Catalogue values long enough to be a sentence, with any placeholder cut off.
fn collect(node: &serde_json::Value, into: &mut Vec<String>) {
    match node {
        serde_json::Value::Object(object) => {
            for child in object.values() {
                collect(child, into);
            }
        }
        serde_json::Value::String(text) => {
            let prose = text.split('{').next().unwrap_or_default().trim();
            if prose.len() >= 20 {
                into.push(prose.to_owned());
            }
        }
        _ => {}
    }
}

#[test]
fn the_replaced_note_appears_only_when_something_was_replaced_and_carries_the_names() {
    let inventory = bundle::inventory(&input()).expect("inventory");
    let configuration = &inventory.entries[1];
    let replaced = configuration
        .redactions
        .iter()
        .find(|note| note.code == notes::REDACTION_CONFIGURATION_REPLACED)
        .expect("the replaced note");
    assert_eq!(
        replaced
            .params
            .get(notes::FIELDS_PARAMETER)
            .map(String::as_str),
        Some("admin_password, api_key"),
        "the field names travel as data, not inside a sentence"
    );
    assert!(replaced.text.contains("admin_password, api_key"));

    let mut nothing_to_replace = input();
    nothing_to_replace.configuration = serde_json::json!({ "max_active_files": 3 });
    let quiet = bundle::inventory(&nothing_to_replace).expect("inventory");
    assert!(
        quiet.entries[1]
            .redactions
            .iter()
            .all(|note| note.code != notes::REDACTION_CONFIGURATION_REPLACED),
        "nothing replaced, nothing said"
    );
    assert_eq!(quiet.digest, inventory.digest, "and the digest is unmoved");
}

/// The digest covers the identity of the entries and nothing a person reads, so two people in
/// two languages approve the same bundle. Pinned to a literal: a future field folded into the
/// digest fails here rather than quietly changing every approval.
#[test]
fn the_digest_is_the_identity_of_the_entries_and_not_their_language() {
    assert_eq!(
        bundle::inventory(&input()).expect("inventory").digest,
        "812cc807b899471f859be2ff2eca5bf329fb8b0e04d1dda15598f5394db81593"
    );
}

#[test]
fn the_manifest_says_in_english_what_each_file_holds_and_what_was_redacted() {
    let now = Utc
        .with_ymd_and_hms(2026, 9, 22, 12, 0, 0)
        .single()
        .expect("time");
    let ids: Vec<String> = bundle::inventory(&input())
        .expect("inventory")
        .entries
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    let built = bundle::build(&input(), &ids, now).expect("bundle");

    let english = catalogue("en");
    for entry in &built.manifest.entries {
        let code = format!(
            "diagnostics.bundle.entry.{}.description",
            entry.id.replace('-', "_")
        );
        assert_eq!(
            entry.description.as_str(),
            lookup(&english, &code).expect("code"),
            "{} carries no readable description",
            entry.path
        );
    }
    for line in built
        .manifest
        .excluded
        .iter()
        .chain(&built.manifest.redactions)
    {
        assert!(
            !line.contains("diagnostics.bundle."),
            "the manifest shows a raw code instead of a sentence: {line}"
        );
    }
    assert!(
        built
            .manifest
            .redactions
            .iter()
            .any(|line| line.contains("admin_password, api_key")),
        "the manifest names what was replaced"
    );

    // And it is the manifest that was actually written into the archive.
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(built.bytes)).expect("zip");
    let mut text = String::new();
    archive
        .by_name("manifest.json")
        .expect("manifest")
        .read_to_string(&mut text)
        .expect("read");
    let parsed: rd_diagnostics::Manifest = serde_json::from_str(&text).expect("json");
    assert_eq!(parsed, built.manifest);
}
