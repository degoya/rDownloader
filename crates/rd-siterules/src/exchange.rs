//! The exchange file, and the example rules every installation starts with (RD-1230-03).
//!
//! Site rules carry no signature. What one installation exports another imports as it is: a
//! versioned document of rule bodies, each with the switch it had where it was exported, so a
//! colleague's import needs no key and no rework. What protects the importing side is not who
//! wrote a file but what the import shows and enforces: the list of rules before anything is
//! stored, a question before a rule of the same id is replaced, every body parsed and validated
//! like any other rule, and the executor's own bolts (host narrowing, redirect checks, no
//! private address ranges) for every rule whatever its origin.
//!
//! The app brings one such document, [`examples`]: a few rules for sites that publish free
//! software and freely licensed media, switched off, so a person can see how a rule is built
//! without anything being fetched unasked. Rules for other sites are not part of the project.

use serde::{Deserialize, Serialize};

use crate::format::Rule;

/// The layout of the exchange file this build writes and reads. `1` was the export of the
/// bodies alone, before a rule travelled with its switch; it is not read any more.
pub const EXCHANGE_VERSION: u32 = 2;

/// The exchange file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Exchange {
    /// Always [`EXCHANGE_VERSION`] for a file this build reads.
    pub format_version: u32,
    #[serde(default)]
    pub rules: Vec<ExchangeEntry>,
}

/// One rule of an exchange file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeEntry {
    /// The rule's own switch where it was exported.
    #[serde(default)]
    pub enabled: bool,
    /// The body, as [`Rule`] serialises it. Opaque here, so a reader can refuse one body and
    /// keep the others.
    pub rule: serde_json::Value,
}

/// The example list compiled into the binary.
const EXAMPLES: &str = include_str!("../resources/examples.json");

/// The example rules, in the order of the file. Every one is switched off in the file, and
/// a caller installs them switched off whatever the file says.
///
/// A body that does not read or validate is left out rather than failing the caller: the
/// file is part of the build, and `exchange_tests` holds every entry to the full rule check,
/// so this is the safe reading of a case the tests exclude.
#[must_use]
pub fn examples() -> Vec<Rule> {
    let Ok(document) = serde_json::from_str::<Exchange>(EXAMPLES) else {
        return Vec::new();
    };
    document
        .rules
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<Rule>(entry.rule).ok())
        .filter(|rule| rule.validate().is_ok())
        .collect()
}

#[cfg(test)]
#[path = "exchange_tests.rs"]
mod tests;
