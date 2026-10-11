//! LinkFilter rules (RD-1240-09): what the LinkGrabber does with a link the moment it arrives,
//! after JDownloader's LinkFilter.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{CategoryId, IngressSource};

rd_plugin_types::domain_id!(LinkFilterRuleId);

/// What a rule does with a link it matches.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkFilterAction {
    /// The link is hidden in the LinkGrabber: kept, never deleted, shown again by the list's
    /// "Show hidden" switch, and left behind by an enqueue that does not name it.
    #[default]
    Hide,
    /// The link is kept as it is, and no later rule is asked: an exception above a broader
    /// hiding rule.
    Accept,
    /// The link is put into the package `package_name` and/or the category `category_id`.
    Route,
}

/// How a rule's `name_pattern` is read.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkFilterNameSyntax {
    /// `*` any run of characters, `?` one character, the whole name, case ignored.
    #[default]
    Glob,
    /// A regular expression, searched in the name as it is written.
    Regex,
}

/// One LinkFilter rule. The enabled rules are asked in `position` order and the first whose
/// conditions all hold decides; a condition left empty holds for every link.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LinkFilterRule {
    pub id: LinkFilterRuleId,
    pub name: String,
    /// Evaluation order, ascending; the list is numbered 1..n.
    pub position: i64,
    pub enabled: bool,
    /// Matched against the link's file name; a link without one never matches.
    #[serde(default)]
    pub name_pattern: Option<String>,
    #[serde(default)]
    pub name_syntax: LinkFilterNameSyntax,
    /// Smallest size in bytes; a link whose size is not known yet never matches a size bound.
    #[serde(default)]
    pub size_min: Option<u64>,
    /// Largest size in bytes, inclusive.
    #[serde(default)]
    pub size_max: Option<u64>,
    /// File types, lower case without the dot (`rar`, `part1.rar`); one matching is enough.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// The host, lower case; its subdomains match too.
    #[serde(default)]
    pub hoster: Option<String>,
    /// Only links that arrived this way.
    #[serde(default)]
    pub source: Option<IngressSource>,
    pub action: LinkFilterAction,
    /// The package a `route` rule puts the link in.
    #[serde(default)]
    pub package_name: Option<String>,
    /// The category a `route` rule gives the link's package; `None` once that category is gone.
    #[serde(default)]
    pub category_id: Option<CategoryId>,
}
