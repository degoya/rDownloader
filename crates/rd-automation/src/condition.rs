//! The condition tree and the normalised event it is evaluated against.
//!
//! The matching rules are the ones the routing rules already use — case-insensitive, a
//! criterion that is not set does not narrow anything, regexes compiled per evaluation — so
//! a person who has written a category rule already knows how an automation condition
//! behaves. What is added here is composition: `all`, `any` and `not`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A field of the event an automation can look at.
///
/// Deliberately small and flat. Everything here is either already on the event or cheap to
/// look up once per run; nothing invites the engine to go and fetch more of the domain
/// while a condition is being evaluated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// Where the link came in from: `manual`, `clipboard`, `api`, `hot_folder`, …
    Source,
    /// Host of the source URL.
    Domain,
    /// File extension without the dot.
    Extension,
    /// File or package name.
    Name,
    /// Category name.
    Category,
    /// Download or package state.
    State,
    /// Stable failure code of a failed download.
    FailureCode,
    /// Total size in bytes.
    SizeBytes,
    /// Download kind: `http`, `usenet`, `torrent`, `media`, …
    Kind,
}

impl Field {
    /// Whether the field holds a number rather than text.
    #[must_use]
    pub const fn is_numeric(self) -> bool {
        matches!(self, Self::SizeBytes)
    }
}

/// How a field is compared to a value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Equals,
    Contains,
    StartsWith,
    EndsWith,
    /// Regular expression, anchored nowhere; an invalid pattern is refused at save time.
    Matches,
    GreaterThan,
    LessThan,
}

/// One comparison.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, ToSchema)]
pub struct Predicate {
    pub field: Field,
    pub operator: Operator,
    pub value: String,
}

/// A condition: a comparison, or a combination of conditions.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ConditionNode {
    /// Always true. What a new automation starts as, and what "no condition" means.
    #[default]
    Always,
    // `no_recursion` makes the generated schema reference the type instead of inlining it.
    // Without it the OpenAPI generator walks the tree forever and overflows its stack, which
    // is a build-time crash rather than an error message.
    All {
        #[schema(no_recursion)]
        nodes: Vec<ConditionNode>,
    },
    Any {
        #[schema(no_recursion)]
        nodes: Vec<ConditionNode>,
    },
    Not {
        #[schema(no_recursion)]
        node: Box<ConditionNode>,
    },
    Predicate {
        predicate: Predicate,
    },
}

/// Why a condition cannot be stored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConditionError {
    /// A `matches` predicate carries a pattern the regex engine refuses.
    Regex { value: String },
    /// A numeric operator was used on a text field, or the other way round.
    Operator { field: Field, operator: Operator },
    /// A comparison against a number that is not one.
    Number { value: String },
    /// An empty `all`/`any` list, which would silently mean "always" or "never".
    EmptyGroup,
}

impl std::fmt::Display for ConditionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Regex { value } => write!(f, "not a valid regular expression: {value}"),
            Self::Operator { field, operator } => {
                write!(f, "{operator:?} cannot be used on {field:?}")
            }
            Self::Number { value } => write!(f, "not a number: {value}"),
            Self::EmptyGroup => f.write_str("a group needs at least one condition"),
        }
    }
}

impl ConditionNode {
    /// Nesting depth, counting a bare predicate as one.
    #[must_use]
    pub fn depth(&self) -> usize {
        match self {
            Self::Always | Self::Predicate { .. } => 1,
            Self::Not { node } => 1 + node.depth(),
            Self::All { nodes } | Self::Any { nodes } => {
                1 + nodes.iter().map(Self::depth).max().unwrap_or(0)
            }
        }
    }

    /// Checks every predicate so an unevaluatable condition never reaches the database.
    pub fn validate(&self) -> Result<(), ConditionError> {
        match self {
            Self::Always => Ok(()),
            Self::Not { node } => node.validate(),
            Self::All { nodes } | Self::Any { nodes } => {
                if nodes.is_empty() {
                    return Err(ConditionError::EmptyGroup);
                }
                nodes.iter().try_for_each(Self::validate)
            }
            Self::Predicate { predicate } => predicate.validate(),
        }
    }

    /// Whether the event satisfies this condition.
    #[must_use]
    pub fn matches(&self, event: &EventContext) -> bool {
        match self {
            Self::Always => true,
            Self::Not { node } => !node.matches(event),
            Self::All { nodes } => nodes.iter().all(|node| node.matches(event)),
            Self::Any { nodes } => nodes.iter().any(|node| node.matches(event)),
            Self::Predicate { predicate } => predicate.matches(event),
        }
    }
}

impl Predicate {
    fn validate(&self) -> Result<(), ConditionError> {
        let numeric_operator = matches!(self.operator, Operator::GreaterThan | Operator::LessThan);
        if numeric_operator != self.field.is_numeric() {
            return Err(ConditionError::Operator {
                field: self.field,
                operator: self.operator,
            });
        }
        if numeric_operator && self.value.trim().parse::<u64>().is_err() {
            return Err(ConditionError::Number {
                value: self.value.clone(),
            });
        }
        if self.operator == Operator::Matches && regex::Regex::new(&self.value).is_err() {
            return Err(ConditionError::Regex {
                value: self.value.clone(),
            });
        }
        Ok(())
    }

    fn matches(&self, event: &EventContext) -> bool {
        if self.field.is_numeric() {
            let Some(actual) = event.numbers.get(&self.field).copied() else {
                return false;
            };
            let Ok(expected) = self.value.trim().parse::<u64>() else {
                return false;
            };
            return match self.operator {
                Operator::GreaterThan => actual > expected,
                Operator::LessThan => actual < expected,
                _ => false,
            };
        }
        // A field the event does not carry never matches — not even a `not equals` written
        // as `not(equals)`, because the enclosing `Not` inverts the result and that is where
        // "the event says nothing about this" is supposed to be decided by the author.
        let Some(actual) = event.text.get(&self.field) else {
            return false;
        };
        let actual = actual.to_lowercase();
        let expected = self.value.to_lowercase();
        match self.operator {
            Operator::Equals => actual == expected,
            Operator::Contains => actual.contains(&expected),
            Operator::StartsWith => actual.starts_with(&expected),
            Operator::EndsWith => actual.ends_with(&expected),
            // Compiled per evaluation, as the routing rules do. An invalid pattern cannot
            // get here: it is refused at save time.
            Operator::Matches => regex::Regex::new(&self.value)
                .is_ok_and(|pattern| pattern.is_match(actual.as_str())),
            Operator::GreaterThan | Operator::LessThan => false,
        }
    }
}

/// The normalised view of an event that conditions are evaluated against.
///
/// Built once per run. Whatever the engine could not determine is simply absent, and an
/// absent field never satisfies a predicate.
#[derive(Clone, Debug, Default)]
pub struct EventContext {
    pub text: BTreeMap<Field, String>,
    pub numbers: BTreeMap<Field, u64>,
}

impl EventContext {
    /// Records a text field, ignoring an empty value so "" cannot match `contains`.
    pub fn set(&mut self, field: Field, value: impl Into<String>) {
        let value = value.into();
        if !value.trim().is_empty() {
            self.text.insert(field, value);
        }
    }

    /// Records a numeric field.
    pub fn set_number(&mut self, field: Field, value: u64) {
        self.numbers.insert(field, value);
    }
}
