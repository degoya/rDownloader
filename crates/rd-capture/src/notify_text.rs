//! The words of the intake toast, in the agent's language (RD-1270-05).
//!
//! The agent carries no message catalogue for its menu (RD-092-05), which stays English. The
//! intake toast is different: it is the one notice the agent raises for links somebody else
//! added, and "8 links added..." in English on a German desktop read like a foreign program. So
//! it speaks the four languages every surface of the interface ships
//! (`web/src/locales/languages.json`), from `crates/rd-capture/locales/<language>.json`, compiled
//! in: the desktop's language as [`super::locale::current`] reads it, English for any other.

use std::{collections::HashMap, sync::OnceLock};

use super::{Intake, locale::Lang};

/// One language's sentences, by key.
type Catalogue = HashMap<String, String>;

fn source(lang: Lang) -> &'static str {
    match lang {
        Lang::En => include_str!("../locales/en.json"),
        Lang::De => include_str!("../locales/de.json"),
        Lang::Es => include_str!("../locales/es.json"),
        Lang::Fr => include_str!("../locales/fr.json"),
    }
}

/// The catalogue of `lang`, read once. One that does not parse is empty, and every sentence
/// then falls back to English; the tests below keep that from shipping.
fn catalogue(lang: Lang) -> &'static Catalogue {
    static CATALOGUES: OnceLock<[Catalogue; 4]> = OnceLock::new();
    let all = CATALOGUES.get_or_init(|| {
        Lang::ALL.map(|lang| serde_json::from_str(source(lang)).unwrap_or_default())
    });
    &all[lang.index()]
}

/// The sentence `key` in `lang` with its `{placeholders}` filled; English where `lang` lacks it.
fn say(lang: Lang, key: &str, values: &[(&str, String)]) -> String {
    let template = catalogue(lang)
        .get(key)
        .or_else(|| catalogue(Lang::En).get(key))
        .map_or(key, String::as_str);
    values
        .iter()
        .fold(template.to_owned(), |text, (name, value)| {
            text.replace(&format!("{{{name}}}"), value)
        })
}

/// The toast for everything one window brought, or for what quiet hours held back.
pub(super) fn summary(intake: &Intake, lang: Lang) -> String {
    let text = counts(intake, lang);
    if intake.held {
        say(lang, "held", &[("text", text)])
    } else {
        text
    }
}

fn counts(intake: &Intake, lang: Lang) -> String {
    // An NZB that arrived on its own: the files inside it, never called links.
    if intake.file_count > 0 && intake.candidate_count == 0 {
        return match intake.file_count {
            1 => say(lang, "nzb_one", &[]),
            files => say(lang, "nzb_many", &[("files", files.to_string())]),
        };
    }
    match (intake.candidate_count, intake.package_count) {
        (0, _) => say(lang, "links_none", &[]),
        (1, _) => say(lang, "links_one", &[]),
        (links, packages) if packages > 1 => say(
            lang,
            "links_packages",
            &[
                ("links", links.to_string()),
                ("packages", packages.to_string()),
            ],
        ),
        (links, _) => say(lang, "links_many", &[("links", links.to_string())]),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{super::Intake, Catalogue, Lang, source, summary};

    fn links(candidate_count: u32, package_count: u32) -> Intake {
        Intake {
            candidate_count,
            package_count,
            ..Intake::default()
        }
    }

    #[test]
    fn a_payload_the_agent_does_not_understand_still_notifies() {
        assert_eq!(
            summary(&Intake::default(), Lang::En),
            "Links added to the LinkGrabber"
        );
    }

    #[test]
    fn the_wording_follows_the_number_of_links_and_packages() {
        assert_eq!(
            summary(&links(1, 1), Lang::En),
            "1 link added to the LinkGrabber"
        );
        assert_eq!(
            summary(&links(8, 1), Lang::En),
            "8 links added to the LinkGrabber"
        );
        assert_eq!(
            summary(&links(8, 2), Lang::En),
            "8 links in 2 packages added to the LinkGrabber"
        );
    }

    #[test]
    fn an_nzb_that_arrived_on_its_own_reports_files_not_links() {
        // The files inside one container are not links somebody added; wording them as such
        // is what made a single drop read like several separate intakes.
        let nzb = |file_count| Intake {
            package_count: 1,
            file_count,
            ..Intake::default()
        };
        assert_eq!(
            summary(&nzb(5), Lang::En),
            "NZB added to the LinkGrabber, 5 files"
        );
        assert_eq!(
            summary(&nzb(1), Lang::En),
            "NZB added to the LinkGrabber, 1 file"
        );
    }

    /// Every language carries every sentence with the placeholders the English one has, so no
    /// toast falls back to English or shows a `{placeholder}` unfilled.
    #[test]
    fn every_catalogue_has_every_sentence_and_its_placeholders() {
        let placeholders = |text: &str| {
            text.split('{')
                .skip(1)
                .filter_map(|rest| rest.split_once('}').map(|(name, _)| name.to_owned()))
                .collect::<BTreeSet<_>>()
        };
        let read = |lang| {
            serde_json::from_str::<Catalogue>(source(lang))
                .unwrap_or_else(|error| panic!("{lang:?} does not parse: {error}"))
        };
        let english = read(Lang::En);
        for lang in Lang::ALL {
            let catalogue = read(lang);
            assert_eq!(
                catalogue.keys().collect::<BTreeSet<_>>(),
                english.keys().collect::<BTreeSet<_>>(),
                "{lang:?}"
            );
            for (key, text) in &english {
                assert_eq!(
                    placeholders(&catalogue[key]),
                    placeholders(text),
                    "{lang:?} {key}"
                );
            }
        }
    }

    /// RD-1270-05: the toast is said in the desktop's language, not in English for everyone.
    #[test]
    fn the_toast_is_said_in_the_agents_language() {
        let held = Intake {
            held: true,
            ..links(8, 2)
        };
        for lang in [Lang::De, Lang::Es, Lang::Fr] {
            for intake in [links(0, 0), links(1, 1), links(8, 1), held] {
                let said = summary(&intake, lang);
                assert_ne!(said, summary(&intake, Lang::En), "{lang:?}");
                assert!(!said.contains('{'), "{lang:?}: {said}");
            }
            assert!(summary(&held, lang).contains('8'), "{lang:?}");
            assert!(summary(&held, lang).contains('2'), "{lang:?}");
        }
    }
}
