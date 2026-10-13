//! Completing a bundled plugin's manifest from the fields every bundled plugin shares
//! (RD-1280-18).
//!
//! Eight fields were the same in all 75 bundled manifests: the manifest revision, the contract
//! version, the signing key's name and public half, and the author, homepage, licence and
//! lowest core version. They live once in `plugins/metadata.toml` now, and `rd-pack plugin
//! package --defaults plugins/metadata.toml` writes each one a manifest leaves out into the
//! copy it packages and signs. What the package carries is therefore a complete manifest, as a
//! third party's always was; a package that lacks one of the fields is refused exactly as
//! before (`validate_manifest`).
//!
//! The completion is textual and deterministic, because `scripts/build-plugins.sh` asks the
//! same question without a compiler: whether a signed package of the same version still holds
//! what would be packaged now (RD-120-47, `completed_manifest` in `scripts/lib/plugin-drift.sh`,
//! held to this function by `artifact_tests.rs`). The rule:
//!
//! - every top-level field the manifest leaves out is taken from the defaults file's top level,
//!   as its line stands there, in the file's order, and written above the manifest under
//!   [`COMPLETED_MARKER`];
//! - every `[metadata]` field it leaves out is written, the same way, directly below the
//!   manifest's own `[metadata]` line;
//! - a signing key supplies `public_key` when neither the manifest nor the defaults name the
//!   same key -- the public half is in the key, its name (`key_id`) is not.
//!
//! A manifest that leaves nothing out, or packaging without a defaults file and a key, keeps
//! every byte.

use std::path::Path;

use anyhow::{Context, Result, bail};

/// The comment line above the top-level fields a completion added.
pub(crate) const COMPLETED_MARKER: &str =
    "# Completed from the shared plugin fields by `rd-pack plugin package` (RD-1280-18).";

/// The top-level fields a defaults file may supply.
const ROOT_FIELDS: [&str; 4] = ["manifest_version", "api_version", "key_id", "public_key"];
/// The `[metadata]` fields a defaults file may supply.
const METADATA_FIELDS: [&str; 5] = [
    "author",
    "homepage",
    "license",
    "min_app_version",
    "support_url",
];

/// One `key = value` line of a defaults file, kept as written.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Field {
    key: String,
    line: String,
}

/// The shared fields of a family of manifests, read from a file such as
/// `plugins/metadata.toml`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManifestDefaults {
    root: Vec<Field>,
    metadata: Vec<Field>,
}

impl ManifestDefaults {
    /// Reads a defaults file: top-level fields, then a `[metadata]` table, each field on one
    /// line. A field no manifest shares -- an id, a version, a capability -- is refused, so
    /// the file cannot quietly become a second manifest.
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str::<toml::Table>(text).context("the manifest defaults are not TOML")?;
        let mut defaults = Self::default();
        let mut in_metadata = false;
        for (number, raw) in text.lines().enumerate() {
            let line = raw.trim_end();
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with('[') {
                if table_header(trimmed) != Some("metadata") {
                    bail!(
                        "line {}: the manifest defaults hold top-level fields and [metadata] only",
                        number + 1
                    );
                }
                in_metadata = true;
                continue;
            }
            let Some(key) = line_key(trimmed) else {
                bail!("line {}: not a `key = value` line", number + 1);
            };
            let (allowed, fields): (&[&str], _) = if in_metadata {
                (&METADATA_FIELDS, &mut defaults.metadata)
            } else {
                (&ROOT_FIELDS, &mut defaults.root)
            };
            if !allowed.contains(&key) {
                bail!(
                    "line {}: `{key}` is not a field plugins share{}",
                    number + 1,
                    if in_metadata { " in [metadata]" } else { "" }
                );
            }
            fields.push(Field {
                key: key.to_owned(),
                line: trimmed.to_owned(),
            });
        }
        Ok(defaults)
    }

    /// Reads and parses the defaults file at `path`.
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read manifest defaults {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("manifest defaults {}", path.display()))
    }
}

/// Writes into `source` every shared field it leaves out; see the module documentation.
///
/// `signing_key` is the base64 public half of the key the package is signed with, if any.
/// The result is checked to be TOML; whether it is a valid manifest is `package_plugin`'s
/// question, asked of exactly these bytes.
pub fn complete_manifest(
    source: &str,
    defaults: Option<&ManifestDefaults>,
    signing_key: Option<&str>,
) -> Result<String> {
    let parsed: toml::Table = toml::from_str(source).context("parse plugin manifest")?;
    let empty = ManifestDefaults::default();
    let defaults = defaults.unwrap_or(&empty);

    let mut root: Vec<String> = defaults
        .root
        .iter()
        .filter(|field| !parsed.contains_key(&field.key))
        .map(|field| field.line.clone())
        .collect();
    if let Some(key) = signing_key
        && !parsed.contains_key("public_key")
    {
        let line = format!("public_key = \"{key}\"");
        match root
            .iter_mut()
            .find(|existing| line_key(existing) == Some("public_key"))
        {
            // The defaults name this very key: their line stands, so a signed package and the
            // script's comparison read the same bytes.
            Some(slot) if line_value(slot) == Some(key) => {}
            Some(slot) => *slot = line,
            None => root.push(line),
        }
    }

    let metadata_table = parsed.get("metadata");
    let present = |key: &str| {
        metadata_table
            .and_then(toml::Value::as_table)
            .is_some_and(|table| table.contains_key(key))
    };
    let metadata: Vec<&str> = defaults
        .metadata
        .iter()
        .filter(|field| !present(&field.key))
        .map(|field| field.line.as_str())
        .collect();

    if root.is_empty() && metadata.is_empty() {
        return Ok(source.to_owned());
    }
    let mut completed = String::with_capacity(source.len() + 512);
    if !root.is_empty() {
        completed.push_str(COMPLETED_MARKER);
        completed.push('\n');
        for line in &root {
            completed.push_str(line);
            completed.push('\n');
        }
    }
    let mut inserted = metadata.is_empty();
    for line in source.split_inclusive('\n') {
        completed.push_str(line);
        if !inserted && table_header(line.trim()) == Some("metadata") {
            if !line.ends_with('\n') {
                completed.push('\n');
            }
            for field in &metadata {
                completed.push_str(field);
                completed.push('\n');
            }
            inserted = true;
        }
    }
    if !inserted {
        if metadata_table.is_some() {
            bail!("write [metadata] as a table of its own to have its shared fields completed");
        }
        if !completed.ends_with('\n') {
            completed.push('\n');
        }
        completed.push_str("\n[metadata]\n");
        for field in &metadata {
            completed.push_str(field);
            completed.push('\n');
        }
    }
    toml::from_str::<toml::Table>(&completed).context("the completed manifest is not TOML")?;
    Ok(completed)
}

/// The name in a `[name]` header line, comment allowed after it; `None` for anything else.
fn table_header(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    if rest.starts_with('[') {
        return None;
    }
    let (name, after) = rest.split_once(']')?;
    let after = after.trim_start();
    (after.is_empty() || after.starts_with('#')).then_some(name.trim())
}

/// The bare key of a `key = value` line.
fn line_key(line: &str) -> Option<&str> {
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    let bare = !key.is_empty()
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));
    bare.then_some(key)
}

/// The string value of a `key = "value"` line.
fn line_value(line: &str) -> Option<&str> {
    let (_, value) = line.split_once('=')?;
    value.trim().strip_prefix('"')?.strip_suffix('"')
}

#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
