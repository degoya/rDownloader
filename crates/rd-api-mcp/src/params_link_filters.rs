//! Parameters of the LinkFilter tools (RD-1240-09); string-typed mirrors of the REST bodies, for
//! the reason `params_config` gives.

use rmcp::schemars;
use serde::Deserialize;

use crate::params_config::IngressSourceParam;

/// What a matching link gets.
#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LinkFilterActionParam {
    /// Kept, but hidden in the LinkGrabber; an enqueue of its package leaves it behind.
    Hide,
    /// Kept as it is, and no later rule is asked.
    Accept,
    /// Put into `package_name` and/or given `category_id`.
    Route,
}

impl From<LinkFilterActionParam> for rd_core::LinkFilterAction {
    fn from(value: LinkFilterActionParam) -> Self {
        match value {
            LinkFilterActionParam::Hide => Self::Hide,
            LinkFilterActionParam::Accept => Self::Accept,
            LinkFilterActionParam::Route => Self::Route,
        }
    }
}

/// How `name_pattern` is read.
#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LinkFilterNameSyntaxParam {
    /// `*` and `?` over the whole file name, case ignored.
    Glob,
    /// A regular expression searched in the file name as written.
    Regex,
}

impl From<LinkFilterNameSyntaxParam> for rd_core::LinkFilterNameSyntax {
    fn from(value: LinkFilterNameSyntaxParam) -> Self {
        match value {
            LinkFilterNameSyntaxParam::Glob => Self::Glob,
            LinkFilterNameSyntaxParam::Regex => Self::Regex,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateLinkFilterParams {
    pub name: String,
    pub action: LinkFilterActionParam,
    pub enabled: Option<bool>,
    /// Matched against the file name; leave it out to match every name.
    pub name_pattern: Option<String>,
    /// `glob` (the default) or `regex`.
    pub name_syntax: Option<LinkFilterNameSyntaxParam>,
    /// Smallest size in bytes; a link whose size is not known yet never matches a size bound.
    pub size_min: Option<u64>,
    /// Largest size in bytes, inclusive.
    pub size_max: Option<u64>,
    /// File types without the dot, e.g. `nfo` or `part1.rar`; one matching is enough.
    pub extensions: Option<Vec<String>>,
    /// Host name; its subdomains match too.
    pub hoster: Option<String>,
    /// Only links that arrived this way.
    pub source: Option<IngressSourceParam>,
    /// For `route`: the package the link goes into.
    pub package_name: Option<String>,
    /// For `route`: the category the link's package gets.
    pub category_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateLinkFilterParams {
    pub id: String,
    pub name: Option<String>,
    pub action: Option<LinkFilterActionParam>,
    pub enabled: Option<bool>,
    pub name_pattern: Option<String>,
    pub name_syntax: Option<LinkFilterNameSyntaxParam>,
    pub size_min: Option<u64>,
    pub size_max: Option<u64>,
    /// Replaces the whole list; an empty list matches every file type.
    pub extensions: Option<Vec<String>>,
    pub hoster: Option<String>,
    pub source: Option<IngressSourceParam>,
    pub package_name: Option<String>,
    pub category_id: Option<String>,
    /// Conditions to drop: name_pattern, size_min, size_max, hoster, source, package_name,
    /// category_id.
    pub clear: Option<Vec<String>>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ReorderLinkFiltersParams {
    /// Rule ids in the order they are to be asked; rules left out follow in their order.
    pub ids: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UnhideCandidatesParams {
    /// LinkGrabber link ids from list_candidates.
    pub candidate_ids: Vec<String>,
}
