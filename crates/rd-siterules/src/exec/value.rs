//! What a step reads and writes: a named value that is either one string or a list of them.
//!
//! A list is not a convenience. `regex` with `all` produces one, `decode` and `redirect` have
//! to work on every element of it, and the links a rule yields are that list at the end. The
//! two shapes are one type so a rule never has to say which it expects.

use std::collections::BTreeMap;

use crate::text::template_variables;

/// The variable a run seeds with the address it was given.
pub const ADDRESS_VARIABLE: &str = "url";
/// The variable a run keeps the last fetched address in.
pub const PAGE_URL_VARIABLE: &str = "page_url";
/// The variable a `captcha` step writes when it names no other.
pub const CAPTCHA_VARIABLE: &str = "captcha";

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
#[derive(Clone, Debug, Default)]
pub struct Variables(BTreeMap<String, Value>);

/// A template named a variable the run has not written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MissingVariable(pub String);

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

    /// Replaces every `${name}` with that variable's first string.
    ///
    /// A template whose placeholders do not close was refused when the rule was loaded, so
    /// the only failure left here is a name nothing has written — which is a rule expecting
    /// a page to have carried something it did not.
    pub fn expand(&self, template: &str) -> Result<String, MissingVariable> {
        let names =
            template_variables(template).ok_or_else(|| MissingVariable(template.to_owned()))?;
        let mut expanded = template.to_owned();
        for name in names {
            let value = self
                .get(name)
                .and_then(Value::first)
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
}
