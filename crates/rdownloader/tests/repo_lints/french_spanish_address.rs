//! The French plugin catalogues say "vous", the Spanish ones "tu" (owner, 2026-10-02).
//!
//! The same heuristic as `web/src/i18n/frenchSpanishAddress.test.ts`, which explains the word
//! lists: French "tu" pronouns, an elided "t'", irregular "tu" imperatives and a hyphenated
//! imperative whose verb does not end in "z"; Spanish "usted", an accented reflexive "usted"
//! imperative and a formal imperative at the start of a clause. A sentence the heuristic misreads
//! is listed by plugin, key and phrase, and the phrase is cut out of that one string only, as in
//! `german_address.rs`. `extension/test/french-spanish-address.test.mjs` holds the rule for the
//! extension.

use crate::{strings, workspace_root};

use serde_json::Value;

const FRENCH_WORDS: &[&str] = &[
    "tu",
    "toi",
    "ton",
    "ta",
    "tes",
    "te",
    "tien",
    "tienne",
    "tiens",
    "tiennes",
    "mets",
    "fais",
    "prends",
    "reprends",
    "attends",
    "vois",
    "lis",
    "relis",
    "\u{e9}cris",
];
const FRENCH_HYPHENATED: &[&str] = &["toi", "moi", "en", "le", "la", "les", "y"];
const SPANISH_WORDS: &[&str] = &["usted", "ustedes"];
const SPANISH_IMPERATIVES: &[&str] = &[
    "abra",
    "acepte",
    "active",
    "actualice",
    "a\u{f1}ada",
    "apruebe",
    "asigne",
    "borre",
    "cambie",
    "cancele",
    "cierre",
    "compare",
    "compruebe",
    "configure",
    "confirme",
    "conecte",
    "consulte",
    "copie",
    "cree",
    "defina",
    "deje",
    "descargue",
    "desactive",
    "desmarque",
    "detenga",
    "ejecute",
    "elija",
    "elimine",
    "escanee",
    "escriba",
    "espere",
    "guarde",
    "haga",
    "indique",
    "inicie",
    "instale",
    "intente",
    "introduzca",
    "marque",
    "mueva",
    "pegue",
    "permita",
    "ponga",
    "pulse",
    "quite",
    "rechace",
    "recargue",
    "reinicie",
    "resuelva",
    "responda",
    "revise",
    "seleccione",
    "use",
    "utilice",
    "vincule",
    "vuelva",
];
const SPANISH_ACCENTED_VOWELS: &str = "\u{e1}\u{e9}\u{ed}\u{f3}\u{fa}";
const CLAUSE_STARTS: &str = ".!?:;\u{2014}\u{2013}\u{bf}\u{a1}(\u{ab}";

/// Misread phrases: locale, plugin directory, key path and the phrase that carries them.
const EXCEPTIONS: &[(&str, &str, &str, &str)] = &[];

/// Every string of every plugin catalogue in `locale` as (plugin, key path, text).
fn catalogue_strings(locale: &str) -> Vec<(String, String, String)> {
    let plugins = workspace_root().join("plugins");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&plugins).expect("plugins directory") {
        let directory = entry.expect("plugin entry").path();
        let catalogue = directory.join("locales").join(format!("{locale}.json"));
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

/// Lower-cased words; letters, digits, apostrophes and hyphens stay inside a word.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '\u{2019}' || c == '-'))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn addresses_as_tu(text: &str) -> bool {
    words(text).iter().any(|word| {
        if FRENCH_WORDS.contains(&word.as_str()) {
            return true;
        }
        let mut chars = word.chars();
        if chars.next() == Some('t')
            && matches!(chars.next(), Some('\'' | '\u{2019}'))
            && chars.next().is_some_and(char::is_alphabetic)
        {
            return true;
        }
        let parts: Vec<&str> = word.split('-').collect();
        match parts.as_slice() {
            [.., verb, pronoun] => {
                FRENCH_HYPHENATED.contains(pronoun) && !verb.is_empty() && !verb.ends_with('z')
            }
            _ => false,
        }
    })
}

fn addresses_as_usted(text: &str) -> bool {
    let formal = words(text).iter().any(|word| {
        SPANISH_WORDS.contains(&word.as_str())
            || (word.chars().any(|c| SPANISH_ACCENTED_VOWELS.contains(c))
                && (word.ends_with("ese") || word.ends_with("ase")))
    });
    formal
        || text
            .split(|c: char| CLAUSE_STARTS.contains(c))
            .any(|clause| {
                let first: String = clause
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphabetic())
                    .collect();
                SPANISH_IMPERATIVES.contains(&first.to_lowercase().as_str())
            })
}

fn offenders(locale: &str, offends: fn(&str) -> bool) -> Vec<String> {
    catalogue_strings(locale)
        .iter()
        .filter(|(plugin, key, text)| {
            let rest = EXCEPTIONS
                .iter()
                .filter(|(l, p, k, _)| *l == locale && *p == plugin.as_str() && *k == key.as_str())
                .fold(text.clone(), |rest, (_, _, _, phrase)| {
                    rest.replacen(*phrase, "", 1)
                });
            offends(&rest)
        })
        .map(|(plugin, key, text)| format!("{plugin} {key}: {text}"))
        .collect()
}

#[test]
fn french_plugin_texts_never_address_the_reader_as_tu() {
    assert!(
        catalogue_strings("fr").len() > 200,
        "expected to read the plugin catalogues"
    );
    let found = offenders("fr", addresses_as_tu);
    assert!(
        found.is_empty(),
        "\"tu\" in a French plugin catalogue:\n{}",
        found.join("\n")
    );
}

#[test]
fn spanish_plugin_texts_never_address_the_reader_as_usted() {
    assert!(
        catalogue_strings("es").len() > 200,
        "expected to read the plugin catalogues"
    );
    let found = offenders("es", addresses_as_usted);
    assert!(
        found.is_empty(),
        "\"usted\" in a Spanish plugin catalogue:\n{}",
        found.join("\n")
    );
}

#[test]
fn the_heuristic_recognises_the_forms_it_is_meant_to_find() {
    assert!(addresses_as_tu("Installe-en un."));
    assert!(addresses_as_tu("Reconnecte-toi."));
    assert!(addresses_as_tu("Si tu renouvelles le jeton."));
    assert!(!addresses_as_tu("Reconnectez-vous et installez-en un."));
    assert!(!addresses_as_tu(
        "V\u{e9}rifie les fichiers .md5, peut-\u{ea}tre."
    ));
    assert!(addresses_as_usted("Introduzca la clave."));
    assert!(addresses_as_usted("Aseg\u{fa}rese de guardar."));
    assert!(addresses_as_usted("Hay una clave. Pulse el bot\u{f3}n."));
    assert!(!addresses_as_usted(
        "Introduce la clave; aseg\u{fa}rate de guardar."
    ));
    assert!(!addresses_as_usted("hasta que el servicio se reinicie"));
}

/// A rewritten sentence leaves its entry behind; the list stays as short as the catalogues need.
#[test]
fn exception_list_names_only_phrases_that_are_still_there() {
    let stale: Vec<String> = EXCEPTIONS
        .iter()
        .filter(|(locale, plugin, key, phrase)| {
            !catalogue_strings(locale).iter().any(|(p, k, text)| {
                p.as_str() == *plugin && k.as_str() == *key && text.contains(*phrase)
            })
        })
        .map(|(locale, plugin, key, phrase)| format!("{locale} {plugin} {key}: {phrase}"))
        .collect();
    assert!(stale.is_empty(), "stale entries:\n{}", stale.join("\n"));
}
