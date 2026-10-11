//! The request bodies of the LinkFilter routes (RD-1240-09) and the one validation the REST
//! routes, the MCP tools and the area import share.

use rd_api_core::input_checks::optional_text;
use rd_core::{
    CandidateId, CategoryId, IngressSource, LinkFilterAction, LinkFilterNameSyntax,
    LinkFilterRuleId,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{ApiError, AppState};

/// The longest name pattern; far beyond any file name a rule could be about.
const MAX_PATTERN_CHARS: usize = 1_000;
/// File types per rule, and the length of one.
const MAX_EXTENSIONS: usize = 32;
const MAX_EXTENSION_CHARS: usize = 32;
/// The longest host name DNS allows.
const MAX_HOSTER_CHARS: usize = 253;
/// The longest package name a rule may give.
const MAX_PACKAGE_CHARS: usize = 255;

/// A LinkFilter rule as the form and the tools send it; the server keeps its place in the order.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LinkFilterRuleRequest {
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// Matched against the file name; empty matches every name.
    #[serde(default)]
    pub name_pattern: Option<String>,
    #[serde(default)]
    pub name_syntax: LinkFilterNameSyntax,
    #[serde(default)]
    pub size_min: Option<u64>,
    #[serde(default)]
    pub size_max: Option<u64>,
    /// File types without the dot, e.g. `nfo`, `part1.rar`.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Host name; its subdomains match too.
    #[serde(default)]
    pub hoster: Option<String>,
    #[serde(default)]
    pub source: Option<IngressSource>,
    pub action: LinkFilterAction,
    /// For `route`: the package the link goes into.
    #[serde(default)]
    pub package_name: Option<String>,
    /// For `route`: the category the link's package gets.
    #[serde(default)]
    pub category_id: Option<CategoryId>,
}

fn enabled_by_default() -> bool {
    true
}

/// The rules in the order they are to be asked; rules left out follow in their current order.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LinkFilterReorderRequest {
    pub ids: Vec<LinkFilterRuleId>,
}

/// What applying the rules to the LinkGrabber changed.
#[derive(Debug, Serialize, ToSchema)]
pub struct LinkFilterApplyResponse {
    /// Links a rule hides now that were shown.
    pub hidden: u64,
    /// Links shown now that a rule hid.
    pub shown: u64,
    /// Links a `route` rule moved into its package or gave its category.
    pub routed: u64,
}

impl From<rd_db::LinkFilterOutcome> for LinkFilterApplyResponse {
    fn from(outcome: rd_db::LinkFilterOutcome) -> Self {
        Self {
            hidden: outcome.hidden,
            shown: outcome.shown,
            routed: outcome.routed,
        }
    }
}

/// Hidden links to show again.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CandidateUnhideRequest {
    pub candidate_ids: Vec<CandidateId>,
}

/// How many of the named links were hidden and are shown now.
#[derive(Debug, Serialize, ToSchema)]
pub struct CandidateUnhideResponse {
    pub shown: u64,
}

/// Checks and normalises a rule: a pattern that compiles, sizes in order, clean file types and
/// host, and a `route` that names where to; a category must exist.
///
/// # Errors
///
/// `400` with a `link_filter.*` code naming the field, `request.name_length` for the name and
/// `category.not_found` for a category this installation does not have.
pub async fn validated_link_filter_rule(
    state: &AppState,
    request: LinkFilterRuleRequest,
) -> Result<rd_db::NewLinkFilterRule, ApiError> {
    rd_api_core::config_fields::validate_name(&request.name)?;
    let name_pattern = optional_text(request.name_pattern);
    if let Some(pattern) = &name_pattern
        && (pattern.chars().count() > MAX_PATTERN_CHARS
            || rd_collector::compile_name_pattern(pattern, request.name_syntax).is_none())
    {
        return Err(ApiError::bad_request(
            "link_filter.name_pattern_invalid",
            "The name pattern is not a valid pattern",
        ));
    }
    if let (Some(minimum), Some(maximum)) = (request.size_min, request.size_max)
        && minimum > maximum
    {
        return Err(ApiError::bad_request(
            "link_filter.size_range_invalid",
            "The smallest size is larger than the largest",
        ));
    }
    let extensions = extensions(request.extensions)?;
    let hoster = hoster(request.hoster)?;
    let (package_name, category_id) = match request.action {
        LinkFilterAction::Route => {
            let package_name = optional_text(request.package_name);
            if package_name.is_none() && request.category_id.is_none() {
                return Err(ApiError::bad_request(
                    "link_filter.route_target_missing",
                    "A route rule needs a package name or a category",
                ));
            }
            if let Some(name) = &package_name {
                rd_api_core::input_checks::name_length(
                    name,
                    "link_filter.package_name_length",
                    MAX_PACKAGE_CHARS,
                )?;
            }
            (package_name, request.category_id)
        }
        // Only a route files a link anywhere; the other two keep nothing they would not use.
        LinkFilterAction::Hide | LinkFilterAction::Accept => (None, None),
    };
    if let Some(category_id) = category_id
        && !state
            .database
            .list_categories()
            .await?
            .iter()
            .any(|category| category.id == category_id)
    {
        return Err(ApiError::bad_request(
            "category.not_found",
            "Category not found",
        ));
    }
    Ok(rd_db::NewLinkFilterRule {
        name: request.name.trim().to_owned(),
        enabled: request.enabled,
        name_pattern,
        name_syntax: request.name_syntax,
        size_min: request.size_min,
        size_max: request.size_max,
        extensions,
        hoster,
        source: request.source,
        action: request.action,
        package_name,
        category_id,
    })
}

/// Lower case without the leading dot, each once, in the order given.
fn extensions(values: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut extensions: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim().trim_start_matches('.').to_lowercase();
        if value.is_empty() || extensions.contains(&value) {
            continue;
        }
        if value.chars().count() > MAX_EXTENSION_CHARS
            || value
                .chars()
                .any(|character| character.is_whitespace() || matches!(character, '/' | '\\'))
        {
            return Err(ApiError::bad_request(
                "link_filter.extension_invalid",
                "A file type must be a short ending without spaces or slashes",
            )
            .with_param("value", value));
        }
        extensions.push(value);
    }
    if extensions.len() > MAX_EXTENSIONS {
        return Err(ApiError::bad_request(
            "link_filter.extension_count",
            "At most 32 file types per rule",
        )
        .with_param("max", MAX_EXTENSIONS));
    }
    Ok(extensions)
}

/// A bare host, lower case: no scheme, port, path or spaces.
fn hoster(value: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(hoster) = optional_text(value) else {
        return Ok(None);
    };
    let hoster = hoster.trim_end_matches('.').to_ascii_lowercase();
    if hoster.is_empty()
        || hoster.chars().count() > MAX_HOSTER_CHARS
        || hoster
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '/' | ':' | '@'))
    {
        return Err(ApiError::bad_request(
            "link_filter.hoster_invalid",
            "The hoster must be a host name without scheme, port or path",
        ));
    }
    Ok(Some(hoster))
}

#[cfg(test)]
mod tests {
    use super::{extensions, hoster};

    #[test]
    fn file_types_are_cleaned_and_a_slash_is_refused() {
        let cleaned = extensions(vec![
            ".NFO".to_owned(),
            "nfo".to_owned(),
            " part1.RAR ".to_owned(),
            String::new(),
        ])
        .expect("clean");
        assert_eq!(cleaned, vec!["nfo".to_owned(), "part1.rar".to_owned()]);
        let refused = extensions(vec!["a/b".to_owned()]).expect_err("refused");
        assert_eq!(refused.code(), "link_filter.extension_invalid");
    }

    #[test]
    fn a_hoster_is_a_bare_host() {
        assert_eq!(
            hoster(Some(" Files.Example. ".to_owned())).expect("host"),
            Some("files.example".to_owned())
        );
        assert_eq!(hoster(Some("  ".to_owned())).expect("empty"), None);
        for refused in [
            "https://files.example",
            "files.example:443",
            "files.example/a",
        ] {
            assert_eq!(
                hoster(Some(refused.to_owned())).expect_err(refused).code(),
                "link_filter.hoster_invalid"
            );
        }
    }
}
