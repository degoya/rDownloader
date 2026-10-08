//! What a step reads and writes: a named value that is either one string or a list of them.
//!
//! A list is not a convenience. `regex` with `all` produces one, `decode` and `redirect` have
//! to work on every element of it, and the links a rule yields are that list at the end. The
//! two shapes are one type so a rule never has to say which it expects.

use std::collections::{BTreeMap, BTreeSet};

use crate::text::template_variables;

/// The variable a run seeds with the address it was given.
pub const ADDRESS_VARIABLE: &str = "url";
/// The variable a run keeps the last fetched address in.
pub const PAGE_URL_VARIABLE: &str = "page_url";
/// The variable a `captcha` step writes when it names no other.
pub const CAPTCHA_VARIABLE: &str = "captcha";
/// The variable a run seeds with this installation's own stable value, when the caller hands
/// one over (RD-1170-03): 32 hexadecimal digits, the same for every run of one installation.
/// A page whose script sends a browser fingerprint along gets this instead; it is not one.
pub const DEVICE_VARIABLE: &str = "device_id";

/// One variable's content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    One(String),
    Many(Vec<String>),
}

impl Value {
    /// The first string, or `None` for an empty list.
    #[must_use]
    pub fn first(&self) -> Option<&str> {
        match self {
            Self::One(text) => Some(text.as_str()),
            Self::Many(items) => items.first().map(String::as_str),
        }
    }

    /// Every string, in order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        let (single, many) = match self {
            Self::One(text) => (Some(text.as_str()), [].as_slice()),
            Self::Many(items) => (None, items.as_slice()),
        };
        single.into_iter().chain(many.iter().map(String::as_str))
    }

    /// Whether there is nothing in it. An empty string counts as nothing: a capture that
    /// matched but caught no characters is not a value a later step can use.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::One(text) => text.is_empty(),
            Self::Many(items) => items.is_empty(),
        }
    }

    /// A list built from an iterator, collapsed to a single value when it holds exactly one.
    /// Keeps `${name}` usable after a `regex` that happened to match once.
    pub fn list(items: impl IntoIterator<Item = String>) -> Self {
        let mut items: Vec<String> = items.into_iter().collect();
        match items.len() {
            1 => Self::One(items.remove(0)),
            _ => Self::Many(items),
        }
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::One(text)
    }
}

/// The variables of one run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Variables(BTreeMap<String, Value>);

/// A template named a variable the run has not written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingVariable(pub String);

/// What a template becomes for a step that runs once per entry (RD-180-18).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expansion {
    /// No placeholder holds a list: the one string [`Variables::expand`] builds.
    One(String),
    /// One placeholder holds a list: one string per distinct entry, in order. Empty when the
    /// list is.
    Each(Vec<String>),
}

/// Why [`Variables::expand_each`] could not expand a template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpandError {
    Missing(MissingVariable),
    /// More than one placeholder holds a list. Pairing them or crossing them are both
    /// guesses, and a rule states neither, so the template refuses instead.
    SeveralLists(Vec<String>),
}

impl Variables {
    /// Reads one, or `None` when no step has written it.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// Writes one, replacing what was there.
    pub fn set(&mut self, name: &str, value: impl Into<Value>) {
        self.0.insert(name.to_owned(), value.into());
    }

    /// Removes one, so a later read finds nothing written.
    pub(crate) fn unset(&mut self, name: &str) {
        self.0.remove(name);
    }

    /// Replaces every `${name}` with that variable's first string.
    ///
    /// A template whose placeholders do not close was refused when the rule was loaded, so
    /// the only failure left here is a name nothing has written — which is a rule expecting
    /// a page to have carried something it did not.
    pub fn expand(&self, template: &str) -> Result<String, MissingVariable> {
        self.substitute(template, None)
    }

    /// Expands `template` once per entry when one of its placeholders holds a list, and once
    /// as [`Self::expand`] does when none does.
    ///
    /// A list entry that repeats is expanded once: the same address asked twice in one run
    /// is a cycle, and the answer would be the same. Two different placeholders holding lists
    /// refuse; the same one named twice takes the same entry in both places.
    pub fn expand_each(&self, template: &str) -> Result<Expansion, ExpandError> {
        let names = template_variables(template)
            .ok_or_else(|| ExpandError::Missing(MissingVariable(template.to_owned())))?;
        let mut lists: Vec<&str> = Vec::new();
        for name in names {
            match self.get(name) {
                // Checked up front, so an empty list does not hide a name nothing wrote.
                None => return Err(ExpandError::Missing(MissingVariable(name.to_owned()))),
                Some(Value::Many(_)) if !lists.contains(&name) => lists.push(name),
                Some(_) => {}
            }
        }
        match lists.as_slice() {
            [] => self
                .expand(template)
                .map(Expansion::One)
                .map_err(ExpandError::Missing),
            [list] => {
                let list = *list;
                let mut seen = BTreeSet::new();
                let mut expanded = Vec::new();
                for entry in self.get(list).into_iter().flat_map(|value| value.iter()) {
                    if seen.insert(entry) {
                        expanded.push(
                            self.substitute(template, Some((list, entry)))
                                .map_err(ExpandError::Missing)?,
                        );
                    }
                }
                Ok(Expansion::Each(expanded))
            }
            _ => Err(ExpandError::SeveralLists(
                lists.iter().map(|name| (*name).to_owned()).collect(),
            )),
        }
    }

    /// Replaces every `${name}` with that variable's first string, or with `entry`'s text
    /// for the one variable `entry` names.
    fn substitute(
        &self,
        template: &str,
        entry: Option<(&str, &str)>,
    ) -> Result<String, MissingVariable> {
        let names =
            template_variables(template).ok_or_else(|| MissingVariable(template.to_owned()))?;
        let mut expanded = template.to_owned();
        for name in names {
            let value = match entry {
                Some((list, text)) if list == name => Some(text),
                _ => self.get(name).and_then(Value::first),
            }
            .ok_or_else(|| MissingVariable(name.to_owned()))?;
            expanded = expanded.replace(&format!("${{{name}}}"), value);
        }
        Ok(expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_reads_the_same_whether_it_is_one_or_many() {
        let one = Value::One("a".to_owned());
        let many = Value::Many(vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(one.iter().collect::<Vec<_>>(), ["a"]);
        assert_eq!(many.iter().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(many.first(), Some("a"));
        assert!(Value::Many(Vec::new()).is_empty());
        assert!(Value::One(String::new()).is_empty());
    }

    #[test]
    fn a_list_of_one_collapses_so_a_template_can_use_it() {
        assert_eq!(Value::list(["a".to_owned()]), Value::One("a".to_owned()));
        assert_eq!(Value::list([]), Value::Many(Vec::new()));
    }

    #[test]
    fn a_template_takes_the_first_string_and_refuses_an_unwritten_name() {
        let mut variables = Variables::default();
        variables.set("base", "https://x.test".to_owned());
        variables.set("id", Value::Many(vec!["7".to_owned(), "8".to_owned()]));
        assert_eq!(
            variables.expand("${base}/dl/${id}").expect("expanded"),
            "https://x.test/dl/7"
        );
        assert_eq!(variables.expand("plain").expect("expanded"), "plain");
        assert_eq!(
            variables.expand("${nothing}"),
            Err(MissingVariable("nothing".to_owned()))
        );
    }

    #[test]
    fn expand_each_runs_over_the_one_list_a_template_names() {
        let mut variables = Variables::default();
        variables.set("base", "https://x.test".to_owned());
        variables.set(
            "id",
            Value::Many(vec!["7".to_owned(), "8".to_owned(), "7".to_owned()]),
        );
        variables.set("other", Value::Many(vec!["a".to_owned(), "b".to_owned()]));
        variables.set("none", Value::Many(Vec::new()));
        // No list: what `expand` builds, unchanged.
        assert_eq!(
            variables.expand_each("${base}/dl"),
            Ok(Expansion::One("https://x.test/dl".to_owned()))
        );
        // One list, named twice: one string per distinct entry, the same entry in both places.
        assert_eq!(
            variables.expand_each("${base}/dl/${id}?again=${id}"),
            Ok(Expansion::Each(vec![
                "https://x.test/dl/7?again=7".to_owned(),
                "https://x.test/dl/8?again=8".to_owned(),
            ]))
        );
        assert_eq!(
            variables.expand_each("${base}/dl/${none}"),
            Ok(Expansion::Each(Vec::new()))
        );
        assert_eq!(
            variables.expand_each("${id}/${other}"),
            Err(ExpandError::SeveralLists(vec![
                "id".to_owned(),
                "other".to_owned()
            ]))
        );
        assert_eq!(
            variables.expand_each("${none}/${nothing}"),
            Err(ExpandError::Missing(MissingVariable("nothing".to_owned())))
        );
    }
}
