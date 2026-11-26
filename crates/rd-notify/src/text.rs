//! The built-in wording of a notification, in the language chosen for it (RD-1480-11).
//!
//! Push, mail, Telegram and a webhook's `title` and `body` speak the one language the settings
//! name (`notification_language`), English while none is chosen. The sentences are in
//! `crates/rd-notify/locales/<language>.json`, compiled in, one file per language the interface
//! ships (`web/src/locales/languages.json`). A rule's own wording is the person's and is never
//! translated; what it takes over with `{title}`, `{message}` and `{names}` is.
//!
//! A [`Text`] names its sentence and what fills it, and is said only when a delivery is queued:
//! whoever announces something needs no setting for it, and most events are nobody's news.
//! Codes, details of an error and names stay what they are in every language.

use std::{collections::HashMap, sync::OnceLock};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The language the built-in wording of a notification is written in.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLanguage {
    #[default]
    En,
    De,
    Es,
    Fr,
}

impl NotificationLanguage {
    /// Every language, in the order of [`Self::index`].
    pub const ALL: [Self; 4] = [Self::En, Self::De, Self::Es, Self::Fr];

    const fn index(self) -> usize {
        match self {
            Self::En => 0,
            Self::De => 1,
            Self::Es => 2,
            Self::Fr => 3,
        }
    }

    const fn source(self) -> &'static str {
        match self {
            Self::En => include_str!("../locales/en.json"),
            Self::De => include_str!("../locales/de.json"),
            Self::Es => include_str!("../locales/es.json"),
            Self::Fr => include_str!("../locales/fr.json"),
        }
    }

    /// Whether `count` takes the singular: French says it for zero too.
    const fn is_one(self, count: u64) -> bool {
        match self {
            Self::Fr => count <= 1,
            Self::En | Self::De | Self::Es => count == 1,
        }
    }
}

/// One language's sentences, by key.
type Catalogue = HashMap<String, String>;

/// The sentence `key` in `language`. A catalogue that does not parse is empty, and every sentence
/// then falls back to English; the tests keep that from shipping.
fn sentence(language: NotificationLanguage, key: &str) -> Option<&'static str> {
    static CATALOGUES: OnceLock<[Catalogue; 4]> = OnceLock::new();
    let all = CATALOGUES.get_or_init(|| {
        NotificationLanguage::ALL
            .map(|language| serde_json::from_str(language.source()).unwrap_or_default())
    });
    all[language.index()].get(key).map(String::as_str)
}

/// What fills a hole of a sentence: a name or a number as it is, a sentence of its own, or
/// several, comma-separated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Words {
    Plain(String),
    Text(Text),
    List(Vec<Words>),
}

impl Words {
    #[must_use]
    pub fn say(&self, language: NotificationLanguage) -> String {
        match self {
            Self::Plain(words) => words.clone(),
            Self::Text(text) => text.say(language),
            Self::List(parts) => parts
                .iter()
                .map(|part| part.say(language))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

impl From<&str> for Words {
    fn from(words: &str) -> Self {
        Self::Plain(words.to_owned())
    }
}

impl From<String> for Words {
    fn from(words: String) -> Self {
        Self::Plain(words)
    }
}

impl From<u64> for Words {
    fn from(number: u64) -> Self {
        Self::Plain(number.to_string())
    }
}

impl From<Text> for Words {
    fn from(text: Text) -> Self {
        Self::Text(text)
    }
}

impl From<Vec<Words>> for Words {
    fn from(parts: Vec<Words>) -> Self {
        Self::List(parts)
    }
}

/// One sentence of the catalogues with what fills its `{holes}`, in no language yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Text {
    key: &'static str,
    /// Chooses between `<key>_one` and `<key>_other`, and fills `{count}`.
    count: Option<u64>,
    values: Vec<(&'static str, Words)>,
}

impl Text {
    /// The sentence `key`.
    #[must_use]
    pub const fn new(key: &'static str) -> Self {
        Self {
            key,
            count: None,
            values: Vec::new(),
        }
    }

    /// The sentence `<key>_one` or `<key>_other`, by `count` and the language's plural.
    #[must_use]
    pub fn counted(key: &'static str, count: u64) -> Self {
        Self {
            key,
            count: Some(count),
            values: vec![("count", count.into())],
        }
    }

    /// Fills the hole `{name}`.
    #[must_use]
    pub fn with(mut self, name: &'static str, words: impl Into<Words>) -> Self {
        self.values.push((name, words.into()));
        self
    }

    /// The sentence in `language`, in English where the catalogue lacks it.
    #[must_use]
    pub fn say(&self, language: NotificationLanguage) -> String {
        let template = self
            .template(language)
            .or_else(|| self.template(NotificationLanguage::En))
            .unwrap_or(self.key);
        fill(template, |name| {
            self.values
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, words)| words.say(language))
        })
    }

    fn template(&self, language: NotificationLanguage) -> Option<&'static str> {
        let Some(count) = self.count else {
            return sentence(language, self.key);
        };
        let form = if language.is_one(count) {
            "one"
        } else {
            "other"
        };
        sentence(language, &format!("{}_{form}", self.key))
    }
}

/// `template` with its `{holes}` filled in one pass, so braces in a name are never a hole of
/// their own. A hole nothing fills stays as it is.
fn fill(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once('{') {
        out.push_str(before);
        let Some((name, tail)) = after.split_once('}') else {
            out.push('{');
            out.push_str(after);
            return out;
        };
        match value(name) {
            Some(words) => out.push_str(&words),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
