//! Wire types of the site-rule settings page (RD-110-08).
//!
//! A rule body travels as the JSON `rd_siterules::Rule` serialises, not as a flattened copy of
//! its fields. Two reasons, and both have cost this project a day before: a second Rust
//! mirror of the seven step kinds would have to be changed in step with `rd-siterules` or
//! start lying, and the editor has to hand back exactly what it was given for a rule it did
//! not touch. The boundary still parses every body through `Rule` and validates it before
//! anything is stored, so "opaque on the wire" is not "unchecked".

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

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
    /// What the rule does and how it is built, as its author wrote it (RD-1230-03).
    pub description: Option<String>,
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
    /// Where the rule's current body came from (RD-1200-05).
    pub origin: SiteRuleOriginResponse,
}

/// Where a rule came from (RD-1200-05, RD-1230-03).
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRuleOriginResponse {
    /// `import` (an imported exchange file), `editor`, `mcp`, `example` (one of the examples the
    /// app brings), or `unknown` for a rule stored before 1.20 or from the signed file of 1.20 to
    /// 1.22.
    pub kind: String,
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

/// One link of a group, with the mirror set the rule placed it in (RD-1170-02).
#[derive(Debug, Serialize, ToSchema)]
pub struct TestedGroupLinkResponse {
    pub url: String,
    /// The mirror set within its group, counted from 1: links of one group carrying the same
    /// number are copies of one file. Absent when the link has no copy.
    pub mirror: Option<u32>,
}

/// One package a rule with `groups` produced (RD-1170-02).
#[derive(Debug, Serialize, ToSchema)]
pub struct TestedGroupResponse {
    /// The package name: the group's own, or the rule's when the group's source read none.
    pub name: Option<String>,
    pub links: Vec<TestedGroupLinkResponse>,
}

/// One entry a two-stage rule listed (RD-1170-03): what a person would choose from.
#[derive(Debug, Serialize, ToSchema)]
pub struct TestedEntryResponse {
    /// The release name the group's `package` read from the entry.
    pub label: Option<String>,
    /// What `groups.pick.attributes` read from the entry; a name it did not match is absent.
    pub attributes: std::collections::BTreeMap<String, String>,
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
    /// One package per entry, for a rule with `groups` (RD-1170-02), in the order found.
    /// Empty for every other rule: its links are one package named `package_name`.
    pub groups: Vec<TestedGroupResponse>,
    /// The entries to choose from, for a rule with `groups.pick` (RD-1170-03): the trial runs
    /// the first stage only, so `links` and `groups` are empty and no captcha is asked. Empty
    /// for every other rule.
    pub entries: Vec<TestedEntryResponse>,
    pub links: Vec<TestedLinkResponse>,
    /// How many of them would become candidates.
    pub kept: usize,
    /// How many answered with a page rather than a file and would be dropped.
    pub refused: usize,
    /// The run's own refusal, when it produced nothing at all.
    pub error: Option<String>,
}

/// The exchange file the export writes and the import reads (RD-1230-03): the person's rules,
/// each with its switch, and nothing else. No signature: what protects the importing side is the
/// preview, the question before a rule is replaced, the full rule check and the executor's own
/// bolts (`docs/security/site-rules.md`).
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SiteRuleDocument {
    /// The layout of this file; `2` is what this build writes and reads.
    pub format_version: u32,
    pub rules: Vec<SiteRuleDocumentEntry>,
}

/// One rule of the exchange file.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SiteRuleDocumentEntry {
    /// The rule's switch where it was exported; the import stores it as it is.
    #[serde(default)]
    pub enabled: bool,
    /// The rule body, as `rd_siterules::Rule` serialises it.
    pub rule: serde_json::Value,
}

/// Which rules the export writes.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SiteRuleExportQuery {
    /// Rule ids separated by commas; every rule when absent. An id no rule carries is skipped.
    pub ids: Option<String>,
}

/// What the import takes: the file, and which of the stored rules it may replace.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct SiteRuleImportRequest {
    pub document: SiteRuleDocument,
    /// Ids of stored rules the person agreed to replace. A rule of the file whose id is stored
    /// and not named here is left as it is (`kept`).
    #[serde(default)]
    pub replace: Vec<String>,
}

/// What became of one rule of the file, or what would become of it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportedSiteRuleResponse {
    /// The rule's id, or the empty string when the body carries none this build can read.
    pub id: String,
    pub name: String,
    /// The hosts the rule claims; empty for a body that does not read.
    pub hosts: Vec<String>,
    /// The switch the rule carries in the file.
    pub enabled: bool,
    /// The preview answers `new`, `replaces` (a stored rule of the same id differs), `same` (a
    /// stored rule is identical, switch included) or `refused`; the import `stored`,
    /// `replaced`, `kept` (a stored rule of the same id was not to be replaced), `same` or
    /// `refused`.
    pub status: String,
    /// Why it was refused, as a stable code.
    pub code: Option<String>,
}

/// What an import would do, before anything is stored.
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRuleImportPreviewResponse {
    pub rules: Vec<ImportedSiteRuleResponse>,
}

/// The result of an import: every rule of the file, and how many were written.
#[derive(Debug, Serialize, ToSchema)]
pub struct ImportSiteRulesResponse {
    pub rules: Vec<ImportedSiteRuleResponse>,
    /// Rules that were new here.
    pub stored: usize,
    /// Stored rules the file replaced.
    pub replaced: usize,
}

/// Deleting every site rule (RD-1230-03).
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct SiteRulesClearRequest {
    /// `true`, or the request is refused with `site_rules.not_confirmed`.
    #[serde(default)]
    pub confirmed: bool,
}

/// What deleting every site rule removed.
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRulesClearResponse {
    /// How many rules went; their self-test results went with them.
    pub removed: u64,
}

/// What restoring the example list wrote.
#[derive(Debug, Serialize, ToSchema)]
pub struct SiteRuleExamplesResponse {
    /// Examples written again, switched off; an example whose id a stored rule carries is left
    /// as it is.
    pub restored: usize,
}
