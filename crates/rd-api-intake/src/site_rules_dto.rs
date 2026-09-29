//! Wire types of the site-rule settings page (RD-110-08).
//!
//! A rule body travels as the JSON `rd_siterules::Rule` serialises, not as a flattened copy of
//! its fields. Two reasons, and both have cost this project a day before: a second Rust
//! mirror of the seven step kinds would have to be changed in step with `rd-siterules` or
//! start lying, and the editor has to hand back exactly what it was given for a rule it did
//! not touch. The boundary still parses every body through `Rule` and validates it before
//! anything is stored, so "opaque on the wire" is not "unchecked".

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What the last self-test said about one rule (RD-110-09).
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRuleCheckResponse {
    /// `ok`, `structural`, `blocked` or `dead`.
    pub verdict: String,
    /// The refusal's stable code, absent exactly when the rule answered.
    pub code: Option<String>,
    pub links: i64,
    pub pages: i64,
    pub checked_at: String,
}

/// One rule as the list shows it.
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRuleResponse {
    pub id: String,
    pub name: String,
    pub group: String,
    /// The hosts the rule claims, `match.hosts` verbatim.
    pub hosts: Vec<String>,
    pub version: u32,
    pub probe: String,
    pub mirrors: bool,
    /// How many steps the rule takes, for the row.
    pub steps: usize,
    /// The switch on the rule itself.
    pub enabled: bool,
    /// Whether the rule is actually consulted: its own switch **and** its group's.
    pub active: bool,
    /// The rule body, as `rd_siterules::Rule` serialises it, so the editor can open it.
    pub rule: serde_json::Value,
    pub check: Option<SiteRuleCheckResponse>,
}

/// One group, with its own switch.
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRuleGroupResponse {
    pub group: String,
    pub enabled: bool,
    /// How many rules carry this group.
    pub rules: usize,
}

/// The whole list, ordered by group and then by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRulesResponse {
    pub rules: Vec<SiteRuleResponse>,
    pub groups: Vec<SiteRuleGroupResponse>,
}

/// Switching one rule or one group.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SiteRuleSwitchRequest {
    pub enabled: bool,
}

/// Writing one of the person's own rules.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SaveSiteRuleRequest {
    /// The rule body. Parsed through `rd_siterules::Rule` and validated before it is stored.
    pub rule: serde_json::Value,
    /// Whether it is consulted. A rule created from the editor may be on right away; an
    /// imported one may not, and the import endpoint never asks for this.
    #[serde(default)]
    pub enabled: bool,
}

/// A trial run against a real address, before the rule is saved.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TestSiteRuleRequest {
    /// The rule body to try. Need not be stored, and is not stored by this call.
    pub rule: serde_json::Value,
    /// The address the person named. Must be one the rule claims.
    pub address: String,
}

/// One address the run produced, with what this installation makes of it.
#[derive(Debug, Serialize, ToSchema)]
pub struct TestedLinkResponse {
    pub url: String,
    /// `claimed`, `confirmed`, `not-a-file` or `unconfirmed` (RD-110-07).
    pub verdict: String,
    /// The stable code of a refused address, absent when it was kept.
    pub code: Option<String>,
}

/// What the trial run found.
#[derive(Debug, Serialize, ToSchema)]
pub struct TestSiteRuleResponse {
    /// The address that was actually crawled -- the one given, or the canonical host it was
    /// revived onto when it arrived on a dead domain.
    pub address: String,
    pub package_name: Option<String>,
    pub pages_fetched: u32,
    pub mirrors: bool,
    pub links: Vec<TestedLinkResponse>,
    /// How many of them would become candidates.
    pub kept: usize,
    /// How many answered with a page rather than a file and would be dropped.
    pub refused: usize,
    /// The run's own refusal, when it produced nothing at all.
    pub error: Option<String>,
}

/// The exchange format of the export and the unsigned import: the person's own rules and
/// nothing else.
///
/// Deliberately not a `RulePack`: a pack is a signed document under its own trust root, and a
/// file somebody was sent is not one. Calling it a pack would invite the two to be confused
/// at exactly the boundary where the difference matters -- which is why the import tells the
/// two apart by the envelope, never by what the payload claims (RD-130-07).
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SiteRuleDocument {
    /// The rule format these bodies are written in; `1` is what this build reads.
    pub format_version: u32,
    /// The rule bodies, as `rd_siterules::Rule` serialises them.
    pub rules: Vec<serde_json::Value>,
}

/// One signature of a [`SignedSiteRuleFile`], as `rd_sign::DocumentSignature` writes it.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SiteRuleFileSignature {
    /// The trusted key the signature claims, `rdownloader-siterules-v1` for the project's file.
    pub key_id: String,
    /// Always `ed25519`.
    pub algorithm: String,
    /// Base64 of the raw 64-byte signature.
    pub signature: String,
}

/// The signed rule file every release carries (RD-130-07): an `rd_sign` envelope over a
/// rule pack.
///
/// Described here for the contract only. The import reads the request's bytes itself,
/// because the signature covers the payload exactly as it arrived and a parsed and
/// re-serialised copy would no longer be what was signed.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SignedSiteRuleFile {
    /// The rule pack as signed: `format_version`, `sequence`, `issued_at` and `rules`.
    pub payload: serde_json::Value,
    pub signatures: Vec<SiteRuleFileSignature>,
}

/// What the import accepts: the signed file of a release, or an export of somebody's own
/// rules. A body carrying `signatures` is read as the first and nothing else.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(untagged)]
pub enum SiteRuleImportRequest {
    Signed(SignedSiteRuleFile),
    Document(SiteRuleDocument),
}

/// What became of one rule in an import.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportedSiteRuleResponse {
    /// The rule's id, or the empty string when the body carries none this build can read.
    pub id: String,
    pub name: String,
    /// `stored` or `refused`.
    pub status: String,
    /// Why it was refused, as a stable code.
    pub code: Option<String>,
}

/// The result of an import: every rule, and how many were stored switched off.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportSiteRulesResponse {
    pub rules: Vec<ImportedSiteRuleResponse>,
    pub stored: usize,
    /// Whether the file was the signed one and its signature held.
    pub signed: bool,
}
