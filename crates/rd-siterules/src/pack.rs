//! The signed pack: the envelope the project's rules travel in, and the order in which it is
//! checked.
//!
//! Since RD-130-07 no pack is compiled into the binary. The project's rules are a release
//! artifact, `rdownloader-site-rules.json`, signed from `resources/site-rules-payload.json`
//! into `resources/site-rules.json`; an installation takes them through the settings page's
//! import, which calls [`verify`] and stores what it read as the person's own rules, switched
//! off.
//!
//! Order matters, and it is: envelope, revocation and signature, format version, freshness,
//! every rule, duplicate ids. A pack that fails at any of these yields no rule at all —
//! not the valid rules before the failing one — because a partially read pack is one whose
//! contents nobody can name.
//!
//! The format version is checked *after* the signature: an unsigned document declaring a
//! future version is still an unsigned document, and saying "unknown version" about it
//! would tell an attacker which of two checks they got past.

use chrono::{DateTime, Utc};
use rd_sign::{Role, SignedDocument, VerifyError, replay};
use serde::{Deserialize, Serialize};

use crate::format::{Rule, RuleError};

/// Domain separator for the pack's signatures (`rd_sign::envelope`).
pub const SITE_RULES_DOMAIN: &str = "rdownloader.site-rules.v1";

/// The pack layout this build understands. Anything else is refused as a whole.
pub const FORMAT_VERSION: u32 = 1;

/// The signed payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RulePack {
    /// Always [`FORMAT_VERSION`] for a pack this build accepts.
    pub format_version: u32,
    /// The publisher's monotonic counter. Never goes backwards; see [`rd_sign::replay`].
    pub sequence: u64,
    /// When the publisher says it signed this.
    pub issued_at: DateTime<Utc>,
    /// After this instant the pack is stale even if nothing newer has been seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<DateTime<Utc>>,
    /// The rules, each with a unique `id`.
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// Why a pack was refused. [`code`](Self::code) is what an interface or a log shows.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("the rule pack cannot be read: {0}")]
    Malformed(String),
    #[error("the rule pack is not signed by a trusted key: {0}")]
    Untrusted(String),
    #[error("the rule pack's signature does not verify: {0}")]
    BadSignature(String),
    #[error("the rule pack has been revoked")]
    Revoked,
    #[error("the rule pack declares format version {saw}, this build reads {FORMAT_VERSION}")]
    FormatVersion { saw: u64 },
    #[error("the rule pack is stale: {0}")]
    Stale(#[from] replay::StaleError),
    #[error("rule {id:?} is invalid: {reason}")]
    Rule {
        id: String,
        #[source]
        reason: RuleError,
    },
    #[error("rule id {0:?} appears twice in the pack")]
    DuplicateId(String),
}

impl PackError {
    /// The stable code, translated by the interface.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Malformed(_) => "site_rules.malformed",
            Self::Untrusted(_) => "site_rules.untrusted",
            Self::BadSignature(_) => "site_rules.bad_signature",
            Self::Revoked => "site_rules.revoked",
            Self::FormatVersion { .. } => "site_rules.format_version_unsupported",
            Self::Stale(_) => "site_rules.stale",
            Self::Rule { .. } => "site_rules.invalid_rule",
            Self::DuplicateId(_) => "site_rules.duplicate_id",
        }
    }
}

impl RulePack {
    /// The freshness fields, as [`rd_sign::replay::check`] wants them.
    #[must_use]
    pub fn freshness(&self) -> replay::Freshness {
        replay::Freshness {
            sequence: self.sequence,
            issued_at: self.issued_at,
            not_after: self.not_after,
        }
    }

    /// Refuses a pack with an invalid rule or a repeated id.
    pub fn validate(&self) -> Result<(), PackError> {
        let mut seen = std::collections::BTreeSet::new();
        for rule in &self.rules {
            rule.validate().map_err(|reason| PackError::Rule {
                id: rule.id.clone(),
                reason,
            })?;
            if !seen.insert(rule.id.as_str()) {
                return Err(PackError::DuplicateId(rule.id.clone()));
            }
        }
        Ok(())
    }
}

/// Verifies a signed pack against the compiled-in site-rules root.
///
/// `known_sequence` is the highest sequence this installation has already accepted. The
/// import passes `None`: it turns the pack into the person's own rules rather than keeping
/// the pack, so there is no earlier sequence for a later file to be measured against.
pub fn verify(
    bytes: &[u8],
    known_sequence: Option<u64>,
    now: DateTime<Utc>,
) -> Result<RulePack, PackError> {
    let trust = rd_sign::trust_store_for(Role::SiteRules, now)
        .map_err(|error| PackError::Untrusted(error.to_string()))?;
    verify_with(bytes, &trust, known_sequence, now)
}

/// [`verify`] against an explicit trust store, so a test can prove that another key, a
/// revoked digest or a foreign domain is refused without editing the shipped roots.
pub fn verify_with(
    bytes: &[u8],
    trust: &rd_sign::TrustStore,
    known_sequence: Option<u64>,
    now: DateTime<Utc>,
) -> Result<RulePack, PackError> {
    let document =
        SignedDocument::parse(bytes).map_err(|error| PackError::Malformed(error.to_string()))?;
    let payload: serde_json::Value =
        document
            .verify(SITE_RULES_DOMAIN, trust)
            .map_err(|error| match error {
                VerifyError::UntrustedKey { .. } => PackError::Untrusted(error.to_string()),
                VerifyError::BadSignature { .. } => PackError::BadSignature(error.to_string()),
                VerifyError::Revoked => PackError::Revoked,
                VerifyError::Other(other) => PackError::Malformed(other.to_string()),
            })?;
    let saw = payload
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| PackError::Malformed("format_version is missing".to_owned()))?;
    if saw != u64::from(FORMAT_VERSION) {
        return Err(PackError::FormatVersion { saw });
    }
    let pack: RulePack =
        serde_json::from_value(payload).map_err(|error| PackError::Malformed(error.to_string()))?;
    replay::check(pack.freshness(), known_sequence, now)?;
    pack.validate()?;
    Ok(pack)
}

/// Signs `pack` under `key_id` and returns the document as it is shipped.
pub fn sign(key_id: &str, key: &rd_sign::SigningKey, pack: &RulePack) -> anyhow::Result<Vec<u8>> {
    if pack.format_version != FORMAT_VERSION {
        anyhow::bail!(
            "pack declares format version {}, this build signs version {FORMAT_VERSION}",
            pack.format_version
        );
    }
    pack.validate()?;
    let document = rd_sign::sign_document(SITE_RULES_DOMAIN, key_id, key, pack)?;
    Ok(serde_json::to_vec_pretty(&document)?)
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
