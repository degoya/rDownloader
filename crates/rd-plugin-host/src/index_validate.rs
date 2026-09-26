//! The content rules of [`super::PluginIndex`]: bounds, formats, duplicates, contradictions.

use std::collections::BTreeSet;

use chrono::Duration;

use super::{
    IndexError, IndexPackage, MAX_INDEX_PACKAGES, MAX_PACKAGE_BYTES, MAX_RELEASE_NOTES_CHARS,
    MAX_REVOKED_DIGESTS, MAX_REVOKED_KEYS, MAX_VALIDITY_DAYS, PLUGIN_INDEX_SCHEMA_VERSION,
    Permissions, PluginIndex, Publisher, Revocations,
};
use crate::{format_package_digest, parse_package_digest};

const MAX_NAME_CHARS: usize = 120;
const MAX_KEY_ID_CHARS: usize = 128;
const MAX_ENTRY_CHARS: usize = 256;
const MAX_LIST_ENTRIES: usize = 256;
const MAX_URL_CHARS: usize = 1024;

impl PluginIndex {
    /// Refuses an index this build must not act on: bounds, formats, duplicates, and an index
    /// that offers what it withdraws.
    pub fn validate(&self) -> Result<(), IndexError> {
        if self.schema_version != PLUGIN_INDEX_SCHEMA_VERSION {
            return Err(IndexError::SchemaVersion {
                saw: u64::from(self.schema_version),
            });
        }
        if self.sequence == 0 {
            return Err(invalid("the sequence starts at 1"));
        }
        if self.not_after <= self.issued_at {
            return Err(invalid("not_after is not after issued_at"));
        }
        if self.not_after - self.issued_at > Duration::days(MAX_VALIDITY_DAYS) {
            return Err(invalid(format!(
                "valid for longer than {MAX_VALIDITY_DAYS} days"
            )));
        }
        if self.packages.len() > MAX_INDEX_PACKAGES {
            return Err(invalid(format!("more than {MAX_INDEX_PACKAGES} packages")));
        }
        self.revoked.validate()?;
        let withdrawn: BTreeSet<&str> = self
            .revoked
            .package_digests
            .iter()
            .map(String::as_str)
            .collect();
        let withdrawn_keys: BTreeSet<&str> = self
            .revoked
            .keys
            .iter()
            .map(|key| key.fingerprint.as_str())
            .collect();
        let mut seen = BTreeSet::new();
        for package in &self.packages {
            let entry = format!("{} {}", package.id, package.version);
            package
                .validate()
                .map_err(|reason| invalid(format!("{entry}: {reason}")))?;
            if !seen.insert((package.id, package.version.as_str())) {
                return Err(invalid(format!("{entry} is listed twice")));
            }
            // An index that offers and withdraws the same thing contradicts itself; which half
            // was meant is not something a verifier should guess.
            if withdrawn.contains(package.package_digest.as_str()) {
                return Err(invalid(format!("{entry} is offered and withdrawn")));
            }
            if withdrawn_keys.contains(package.publisher.fingerprint.as_str()) {
                return Err(invalid(format!(
                    "{entry} is offered under a withdrawn signing key"
                )));
            }
        }
        Ok(())
    }
}

impl IndexPackage {
    fn validate(&self) -> Result<(), String> {
        bounded_text("name", &self.name, MAX_NAME_CHARS)?;
        if self.name.trim().is_empty() {
            return Err("the name is empty".to_owned());
        }
        semver_field("version", &self.version)?;
        semver_field("api_version", &self.api_version)?;
        if let Some(minimum) = &self.min_app_version {
            semver_field("min_app_version", minimum)?;
        }
        let kind = self.plugin_type.as_str();
        if kind.is_empty()
            || kind.len() > 32
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        {
            return Err(format!("plugin_type {kind:?} is not a type name"));
        }
        canonical_digest("package_digest", &self.package_digest)?;
        if self.size == 0 || self.size > MAX_PACKAGE_BYTES {
            return Err(format!(
                "declares an implausible size of {} bytes",
                self.size
            ));
        }
        validate_url(&self.url)?;
        self.publisher.validate()?;
        self.permissions.validate()?;
        if let Some(notes) = &self.release_notes {
            if notes.chars().count() > MAX_RELEASE_NOTES_CHARS {
                return Err(format!(
                    "release notes exceed {MAX_RELEASE_NOTES_CHARS} characters"
                ));
            }
            if notes
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
            {
                return Err("release notes carry control characters".to_owned());
            }
        }
        Ok(())
    }
}

impl Publisher {
    fn validate(&self) -> Result<(), String> {
        key_id("publisher key_id", &self.key_id)?;
        canonical_digest("publisher fingerprint", &self.fingerprint)?;
        bounded_text("publisher author", &self.author, MAX_NAME_CHARS)
    }
}

impl Permissions {
    fn validate(&self) -> Result<(), String> {
        for (field, list) in [
            ("granted", &self.granted),
            ("http_domains", &self.http_domains),
            ("stream_hosts", &self.stream_hosts),
        ] {
            if list.len() > MAX_LIST_ENTRIES {
                return Err(format!("{field} has more than {MAX_LIST_ENTRIES} entries"));
            }
            for entry in list {
                bounded_text(field, entry, MAX_ENTRY_CHARS)?;
                if entry.is_empty() {
                    return Err(format!("{field} has an empty entry"));
                }
            }
        }
        Ok(())
    }
}

impl Revocations {
    /// Refuses a list this build must not act on.
    pub fn validate(&self) -> Result<(), IndexError> {
        if self.package_digests.len() > MAX_REVOKED_DIGESTS {
            return Err(invalid(format!(
                "more than {MAX_REVOKED_DIGESTS} withdrawn digests"
            )));
        }
        if self.keys.len() > MAX_REVOKED_KEYS {
            return Err(invalid(format!(
                "more than {MAX_REVOKED_KEYS} withdrawn keys"
            )));
        }
        for digest in &self.package_digests {
            canonical_digest("withdrawn digest", digest).map_err(invalid)?;
        }
        for key in &self.keys {
            key_id("withdrawn key_id", &key.key_id).map_err(invalid)?;
            canonical_digest("withdrawn key fingerprint", &key.fingerprint).map_err(invalid)?;
        }
        Ok(())
    }
}

fn invalid(reason: impl Into<String>) -> IndexError {
    IndexError::Invalid(reason.into())
}

/// Printable text of at most `limit` characters, no control characters.
fn bounded_text(field: &str, value: &str, limit: usize) -> Result<(), String> {
    if value.chars().count() > limit {
        return Err(format!("{field} exceeds {limit} characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field} carries control characters"));
    }
    Ok(())
}

fn key_id(field: &str, value: &str) -> Result<(), String> {
    bounded_text(field, value, MAX_KEY_ID_CHARS)?;
    if value.trim().is_empty() {
        return Err(format!("{field} is empty"));
    }
    Ok(())
}

fn semver_field(field: &str, value: &str) -> Result<(), String> {
    bounded_text(field, value, 64)?;
    semver::Version::parse(value)
        .map(|_| ())
        .map_err(|_| format!("{field} {value:?} is not semantic versioning"))
}

/// A digest or fingerprint in exactly the form `format_package_digest` writes: one spelling
/// per value, so two entries naming the same bytes cannot slip past a set comparison.
fn canonical_digest(field: &str, value: &str) -> Result<(), String> {
    match parse_package_digest(value) {
        Ok(digest) if format_package_digest(&digest) == value => Ok(()),
        _ => Err(format!(
            "{field} is not 64 lowercase hexadecimal characters"
        )),
    }
}

/// An absolute `https://` URL, or a relative path that stays below the index's directory.
///
/// A relative entry is joined onto the index's URL; refusing `/`, `..`, `\`, a scheme, a query
/// and a fragment means the join can only descend from where the index was found.
fn validate_url(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_URL_CHARS {
        return Err(format!("url is empty or longer than {MAX_URL_CHARS} bytes"));
    }
    if value
        .chars()
        .any(|character| character.is_control() || character == ' ')
    {
        return Err("url carries whitespace or control characters".to_owned());
    }
    if value.starts_with("https://") {
        let parsed = url::Url::parse(value).map_err(|error| format!("url: {error}"))?;
        if parsed.host_str().is_none_or(str::is_empty)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err("url needs a host and no credentials".to_owned());
        }
        return Ok(());
    }
    if value.contains([':', '\\', '?', '#'])
        || value.starts_with('/')
        || value
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(format!(
            "url {value:?} is neither https:// nor a plain relative path"
        ));
    }
    Ok(())
}
