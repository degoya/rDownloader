//! The diagnostic bundle: an inventory a person approves, and the archive built from it.
//!
//! Two properties the tests hold:
//!
//! * **The inventory is a function of the state.** Same input, same entries in the same order
//!   with the same digest; the digest covers what an entry *is* (id, path, kind) and not how
//!   many items it happens to hold, so a warning logged between the preview and the approval
//!   does not turn the approval stale.
//! * **The archive is a function of the inventory and the clock.** Same input, same selection
//!   and same `created_at` produce byte-identical archives, because every file is rendered from
//!   sorted structures and every archive entry carries the same fixed modification time.
//!
//! What never goes in, whatever the input holds: file contents, full URLs, secrets. The
//! configuration is scrubbed here, not by the caller, so the rule has one home and one test.
//!
//! Everything a person reads about an entry is a [`Note`] rather than a sentence written here:
//! a stable code the interface translates, plus the English rendering `manifest.json` keeps for
//! a reader who opened the archive without the application (RD-120-15).

use std::io::{Cursor, Write};

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use rd_core::{REDACTION_PLACEHOLDER, is_secret_parameter, redact_text};
use rd_db::LogRecord;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{
    checks::{Check, render},
    notes::{self, Note},
};

/// The manifest format this crate writes; bump when an entry changes meaning.
pub const MANIFEST_FORMAT: u32 = 1;
/// Records `recent-errors.json` holds at most.
pub const RECENT_ERRORS_LIMIT: usize = 200;

/// Everything the bundle can be built from, collected by the caller.
#[derive(Clone, Debug, Default, Serialize)]
pub struct BundleInput {
    pub application_version: String,
    pub os: String,
    pub arch: String,
    /// Installed plugins as `(id, version)`, in any order.
    pub plugins: Vec<(String, String)>,
    /// The settings document as stored; scrubbed here before it is written.
    pub configuration: serde_json::Value,
    pub checks: Vec<Check>,
    /// The newest records at `warn` and above, newest first.
    pub recent_errors: Vec<LogRecord>,
    /// How many records the whole store holds.
    pub log_records_total: u64,
}

/// One file the bundle may contain, as the preview shows it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct InventoryEntry {
    /// The stable id a person selects by.
    pub id: String,
    /// The path inside the archive.
    pub path: String,
    /// `json` or `text`.
    pub kind: String,
    /// What the entry holds, in one sentence, as a code the interface translates.
    pub description: Note,
    /// How many items it holds right now: plugins, checks, records, fields.
    pub items: u64,
    /// What was removed or replaced before the entry was written.
    pub redactions: Vec<Note>,
}

/// The preview a person approves.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct Inventory {
    pub entries: Vec<InventoryEntry>,
    /// What no bundle ever contains, stated so the approval is informed.
    pub excluded: Vec<Note>,
    /// SHA-256 over the entries' ids, paths and kinds; the approval quotes it back.
    pub digest: String,
}

/// One file as it was written.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct ManifestEntry {
    pub id: String,
    pub path: String,
    pub kind: String,
    /// What the file holds, in English: this manifest is read outside the application, by
    /// somebody who has no interface to translate a code for them.
    pub description: String,
    pub bytes: u64,
    pub sha256: String,
}

/// `manifest.json`: what the archive holds and what was left out.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct Manifest {
    pub format: u32,
    pub created_at: String,
    pub application_version: String,
    pub inventory_digest: String,
    pub entries: Vec<ManifestEntry>,
    /// Entries the person deselected.
    pub omitted: Vec<String>,
    /// `<path>: <what was redacted>`, in English, for the same reason.
    pub redactions: Vec<String>,
    /// What no bundle ever contains, in English, for the same reason.
    pub excluded: Vec<String>,
}

/// The archive bytes and the manifest inside them.
#[derive(Clone, Debug)]
pub struct BuiltBundle {
    pub bytes: Vec<u8>,
    pub manifest: Manifest,
}

const EXCLUDED: &[&str] = &[
    notes::EXCLUSION_FILE_CONTENTS,
    notes::EXCLUSION_URLS,
    notes::EXCLUSION_SECRETS,
];

const CONFIGURATION_REDACTIONS: &[&str] = &[
    notes::REDACTION_CONFIGURATION_CREDENTIALS,
    notes::REDACTION_CONFIGURATION_PEM,
    notes::REDACTION_CONFIGURATION_TEXT,
];

const LOG_REDACTIONS: &[&str] = &[notes::REDACTION_LOG_RECORDS];

/// The lines saying what no bundle ever contains, in the order the preview lists them.
fn excluded_notes() -> Vec<Note> {
    EXCLUDED.iter().map(|code| Note::new(code)).collect()
}

/// One JSON file per section of `versions.json`.
#[derive(Serialize)]
struct Versions<'a> {
    application: &'a str,
    os: &'a str,
    arch: &'a str,
    plugins: std::collections::BTreeMap<&'a str, &'a str>,
}

#[derive(Serialize)]
struct RecentErrors<'a> {
    log_records_total: u64,
    limit: usize,
    records: &'a [LogRecord],
}

fn json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).context("render bundle entry")?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Every entry the bundle can hold, rendered, in archive order.
fn render_entries(input: &BundleInput) -> Result<Vec<(InventoryEntry, Vec<u8>)>> {
    let mut plugins: Vec<(&str, &str)> = input
        .plugins
        .iter()
        .map(|(id, version)| (id.as_str(), version.as_str()))
        .collect();
    plugins.sort_unstable();
    let versions = Versions {
        application: &input.application_version,
        os: &input.os,
        arch: &input.arch,
        plugins: plugins.into_iter().collect(),
    };
    let (configuration, scrubbed) = scrub_configuration(&input.configuration);
    let recent: &[LogRecord] =
        &input.recent_errors[..input.recent_errors.len().min(RECENT_ERRORS_LIMIT)];
    let mut configuration_redactions: Vec<Note> = CONFIGURATION_REDACTIONS
        .iter()
        .map(|code| Note::new(code))
        .collect();
    if !scrubbed.is_empty() {
        // The field names are data: they travel as a parameter and are never translated.
        configuration_redactions.push(Note::with(
            notes::REDACTION_CONFIGURATION_REPLACED,
            [(notes::FIELDS_PARAMETER.to_owned(), scrubbed.join(", "))]
                .into_iter()
                .collect(),
        ));
    }
    Ok(vec![
        (
            InventoryEntry {
                id: "versions".to_owned(),
                path: "versions.json".to_owned(),
                kind: "json".to_owned(),
                description: Note::new(notes::ENTRY_VERSIONS),
                items: input.plugins.len() as u64,
                redactions: Vec::new(),
            },
            json(&versions)?,
        ),
        (
            InventoryEntry {
                id: "configuration".to_owned(),
                path: "configuration.json".to_owned(),
                kind: "json".to_owned(),
                description: Note::new(notes::ENTRY_CONFIGURATION),
                items: configuration
                    .as_object()
                    .map_or(0, |object| object.len() as u64),
                redactions: configuration_redactions,
            },
            json(&configuration)?,
        ),
        (
            InventoryEntry {
                id: "system-checks".to_owned(),
                path: "system-checks.json".to_owned(),
                kind: "json".to_owned(),
                description: Note::new(notes::ENTRY_SYSTEM_CHECKS),
                items: input.checks.len() as u64,
                redactions: vec![Note::new(notes::REDACTION_PATHS)],
            },
            json(&input.checks)?,
        ),
        (
            InventoryEntry {
                id: "doctor".to_owned(),
                path: "doctor.txt".to_owned(),
                kind: "text".to_owned(),
                description: Note::new(notes::ENTRY_DOCTOR),
                items: input.checks.len() as u64,
                redactions: vec![Note::new(notes::REDACTION_PATHS)],
            },
            redact_text(&render(&input.checks)).into_bytes(),
        ),
        (
            InventoryEntry {
                id: "recent-errors".to_owned(),
                path: "recent-errors.json".to_owned(),
                kind: "json".to_owned(),
                description: Note::new(notes::ENTRY_RECENT_ERRORS),
                items: recent.len() as u64,
                redactions: LOG_REDACTIONS.iter().map(|code| Note::new(code)).collect(),
            },
            json(&RecentErrors {
                log_records_total: input.log_records_total,
                limit: RECENT_ERRORS_LIMIT,
                records: recent,
            })?,
        ),
    ])
}

/// The approval's fingerprint: what each entry *is*, never what it says.
///
/// Descriptions and redaction notes are deliberately left out. They are codes the interface
/// renders in the reader's language (see [`crate::notes`]), so folding them in would make two
/// people produce two different digests -- and therefore two different approvals -- from one
/// and the same state.
fn digest_of(entries: &[InventoryEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry.id.as_bytes());
        hasher.update(b"\n");
        hasher.update(entry.path.as_bytes());
        hasher.update(b"\n");
        hasher.update(entry.kind.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

/// The preview: what the bundle would hold, and the digest the approval quotes back.
pub fn inventory(input: &BundleInput) -> Result<Inventory> {
    let entries: Vec<InventoryEntry> = render_entries(input)?
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();
    let digest = digest_of(&entries);
    Ok(Inventory {
        entries,
        excluded: excluded_notes(),
        digest,
    })
}

/// Builds the archive from the selected entries. `selected` names entry ids; an id the
/// inventory does not know is refused rather than ignored, because a typo in an approval must
/// not silently produce a different bundle than the one approved.
pub fn build(
    input: &BundleInput,
    selected: &[String],
    created_at: DateTime<Utc>,
) -> Result<BuiltBundle> {
    let rendered = render_entries(input)?;
    let all: Vec<InventoryEntry> = rendered.iter().map(|(entry, _)| entry.clone()).collect();
    for id in selected {
        ensure!(
            all.iter().any(|entry| &entry.id == id),
            "unknown bundle entry {id}"
        );
    }
    let inventory_digest = digest_of(&all);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let mut entries = Vec::new();
    let mut omitted = Vec::new();
    let mut redactions: Vec<String> = Vec::new();
    for (entry, bytes) in &rendered {
        if !selected.contains(&entry.id) {
            omitted.push(entry.id.clone());
            continue;
        }
        writer
            .start_file(entry.path.as_str(), options)
            .with_context(|| format!("start {}", entry.path))?;
        writer
            .write_all(bytes)
            .with_context(|| format!("write {}", entry.path))?;
        entries.push(ManifestEntry {
            id: entry.id.clone(),
            path: entry.path.clone(),
            kind: entry.kind.clone(),
            description: entry.description.text.clone(),
            bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        });
        for redaction in &entry.redactions {
            let line = format!("{}: {}", entry.path, redaction.text);
            if !redactions.contains(&line) {
                redactions.push(line);
            }
        }
    }
    let manifest = Manifest {
        format: MANIFEST_FORMAT,
        created_at: created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        application_version: input.application_version.clone(),
        inventory_digest,
        entries,
        omitted,
        redactions,
        excluded: excluded_notes().into_iter().map(|note| note.text).collect(),
    };
    writer
        .start_file("manifest.json", options)
        .context("start manifest.json")?;
    writer
        .write_all(&json(&manifest)?)
        .context("write manifest.json")?;
    let cursor = writer.finish().context("finish bundle archive")?;
    Ok(BuiltBundle {
        bytes: cursor.into_inner(),
        manifest,
    })
}

/// `rdownloader-diagnostics-<UTC>.zip`, the only name a bundle file ever has.
#[must_use]
pub fn file_name(created_at: DateTime<Utc>) -> String {
    format!(
        "rdownloader-diagnostics-{}.zip",
        created_at.format("%Y%m%dT%H%M%SZ")
    )
}

/// Whether a name is one [`file_name`] could have produced — the whole test a download
/// route needs, since it leaves no room for a separator or a traversal.
#[must_use]
pub fn is_file_name(name: &str) -> bool {
    let Some(stamp) = name
        .strip_prefix("rdownloader-diagnostics-")
        .and_then(|rest| rest.strip_suffix(".zip"))
    else {
        return false;
    };
    stamp.len() == 16
        && stamp.as_bytes()[8] == b'T'
        && stamp.ends_with('Z')
        && stamp
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 15) || byte.is_ascii_digit())
}

/// Removes what the configuration must not carry and returns the names it replaced.
///
/// Recursive over objects and arrays. A key that names a credential is replaced whole; a key
/// holding PEM material is replaced by `[present]`, because whether a certificate is
/// configured matters for a diagnosis and its bytes never do; every other string passes
/// through the text redaction, which strips signed-URL parameters and header values.
#[must_use]
pub fn scrub_configuration(value: &serde_json::Value) -> (serde_json::Value, Vec<String>) {
    let mut replaced = Vec::new();
    let scrubbed = scrub(value, None, &mut replaced);
    replaced.sort_unstable();
    replaced.dedup();
    (scrubbed, replaced)
}

fn is_pem_field(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.ends_with("_pem") || lowered.contains("private_key") || lowered.contains("certificate")
}

fn is_credential_field(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    is_secret_parameter(&lowered)
        || lowered.contains("password")
        || lowered.contains("passphrase")
        || lowered.contains("secret")
        || lowered.contains("token")
        || lowered.contains("api_key")
        || lowered == "cookie"
        || lowered == "cookies"
        || lowered.ends_with("_cookie")
        || lowered.ends_with("_cookies")
}

fn scrub(
    value: &serde_json::Value,
    key: Option<&str>,
    replaced: &mut Vec<String>,
) -> serde_json::Value {
    match value {
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(name, inner)| (name.clone(), scrub(inner, Some(name), replaced)))
                .collect(),
        ),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|inner| scrub(inner, key, replaced))
                .collect(),
        ),
        serde_json::Value::String(text) => {
            if let Some(name) = key {
                if is_pem_field(name) {
                    replaced.push(name.to_owned());
                    return serde_json::Value::String("[present]".to_owned());
                }
                if is_credential_field(name) {
                    replaced.push(name.to_owned());
                    return serde_json::Value::String(REDACTION_PLACEHOLDER.to_owned());
                }
            }
            serde_json::Value::String(redact_text(text))
        }
        // A number, a boolean or a null cannot be a credential, whatever its key says.
        other => other.clone(),
    }
}
