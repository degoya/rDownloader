//! The lines a person reads about the diagnostic bundle, as codes rather than prose.
//!
//! `AGENTS.md`: user-facing text never lives in Rust as prose, it lives as a stable code
//! translated in `web/src/locales/`. The bundle used to break that rule -- the interface drew a
//! translated frame around English sentences that this crate had written, including the lines
//! that say what was redacted (RD-120-15).
//!
//! Every such line is now a [`Note`]: a stable `code` the interface translates, the `params` the
//! sentence interpolates (field names -- data, never translated), and `text`, the same sentence
//! in English.
//!
//! **The English is not written here either.** It is read at compile time out of
//! `web/src/locales/en/logs.json`, the very catalogue the interface translates from, under the
//! path the code spells. That gives the two readers of one inventory what each of them needs:
//! the interface translates the code into the language of the person approving the bundle, and
//! `manifest.json` inside the archive keeps a plain English rendering for whoever opens the zip
//! in a support queue without the application in front of them. Because both come from the same
//! file, they cannot drift, and no user-facing sentence lives in `crates/rd-diagnostics/`.
//!
//! What a note says is deliberately *not* part of the inventory digest -- see
//! [`crate::bundle::inventory`]. The digest covers what an entry is, so the same state produces
//! the same bundle whatever language anybody happens to be reading in.

use std::{collections::BTreeMap, sync::LazyLock};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The English catalogue, embedded. The interface loads the same file.
const ENGLISH_CATALOGUE: &str = include_str!("../../../web/src/locales/en/logs.json");

/// What `versions.json` holds.
pub const ENTRY_VERSIONS: &str = "diagnostics.bundle.entry.versions.description";
/// What `configuration.json` holds.
pub const ENTRY_CONFIGURATION: &str = "diagnostics.bundle.entry.configuration.description";
/// What `system-checks.json` holds.
pub const ENTRY_SYSTEM_CHECKS: &str = "diagnostics.bundle.entry.system_checks.description";
/// What `doctor.txt` holds.
pub const ENTRY_DOCTOR: &str = "diagnostics.bundle.entry.doctor.description";
/// What `recent-errors.json` holds.
pub const ENTRY_RECENT_ERRORS: &str = "diagnostics.bundle.entry.recent_errors.description";

/// Credential-named fields of the configuration are replaced.
pub const REDACTION_CONFIGURATION_CREDENTIALS: &str =
    "diagnostics.bundle.redaction.configuration_credentials";
/// PEM material of the configuration is replaced.
pub const REDACTION_CONFIGURATION_PEM: &str = "diagnostics.bundle.redaction.configuration_pem";
/// Text values of the configuration pass the URL and header redaction.
pub const REDACTION_CONFIGURATION_TEXT: &str = "diagnostics.bundle.redaction.configuration_text";
/// Which field names were actually replaced; carries the `fields` parameter.
pub const REDACTION_CONFIGURATION_REPLACED: &str =
    "diagnostics.bundle.redaction.configuration_replaced";
/// Paths pass the text redaction.
pub const REDACTION_PATHS: &str = "diagnostics.bundle.redaction.paths";
/// Log records were redacted when they were captured.
pub const REDACTION_LOG_RECORDS: &str = "diagnostics.bundle.redaction.log_records";

/// Download file contents and names never go in.
pub const EXCLUSION_FILE_CONTENTS: &str = "diagnostics.bundle.exclusion.file_contents";
/// Full URLs never go in.
pub const EXCLUSION_URLS: &str = "diagnostics.bundle.exclusion.urls";
/// Secrets never go in.
pub const EXCLUSION_SECRETS: &str = "diagnostics.bundle.exclusion.secrets";

/// The parameter name [`REDACTION_CONFIGURATION_REPLACED`] interpolates.
pub const FIELDS_PARAMETER: &str = "fields";

/// Every code this crate can emit, for the tests that hold the catalogues to it.
pub const ALL_CODES: &[&str] = &[
    ENTRY_VERSIONS,
    ENTRY_CONFIGURATION,
    ENTRY_SYSTEM_CHECKS,
    ENTRY_DOCTOR,
    ENTRY_RECENT_ERRORS,
    REDACTION_CONFIGURATION_CREDENTIALS,
    REDACTION_CONFIGURATION_PEM,
    REDACTION_CONFIGURATION_TEXT,
    REDACTION_CONFIGURATION_REPLACED,
    REDACTION_PATHS,
    REDACTION_LOG_RECORDS,
    EXCLUSION_FILE_CONTENTS,
    EXCLUSION_URLS,
    EXCLUSION_SECRETS,
];

/// One sentence a person reads, in both forms its two readers need.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct Note {
    /// The stable code. The interface translates it under `logs.<code>`.
    pub code: String,
    /// The same sentence in English, rendered from the English catalogue with `params`
    /// already interpolated. What the archive carries and what the interface falls back to.
    pub text: String,
    /// Values the sentence interpolates. Data, such as field names, and never translated.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl Note {
    /// A note whose sentence takes no parameter.
    #[must_use]
    pub fn new(code: &str) -> Self {
        Self::with(code, BTreeMap::new())
    }

    /// A note whose sentence interpolates `params`.
    #[must_use]
    pub fn with(code: &str, params: BTreeMap<String, String>) -> Self {
        Self {
            code: code.to_owned(),
            text: english(code, &params),
            params,
        }
    }
}

static ENGLISH: LazyLock<serde_json::Value> =
    LazyLock::new(|| serde_json::from_str(ENGLISH_CATALOGUE).unwrap_or(serde_json::Value::Null));

/// The English rendering of `code`, with `params` interpolated.
///
/// A code the catalogue does not know renders as the code itself rather than as an empty line,
/// so a gap is visible in the archive instead of silent. The tests keep that from shipping.
#[must_use]
pub fn english(code: &str, params: &BTreeMap<String, String>) -> String {
    let mut node = &*ENGLISH;
    for segment in code.split('.') {
        match node.get(segment) {
            Some(child) => node = child,
            None => return code.to_owned(),
        }
    }
    let Some(text) = node.as_str() else {
        return code.to_owned();
    };
    params
        .iter()
        .fold(text.to_owned(), |rendered, (name, value)| {
            rendered.replace(&format!("{{{name}}}"), value)
        })
}
