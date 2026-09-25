use rd_core::{CategoryId, CategoryRule, IngressSource};
use regex::Regex;
use url::Url;

/// Facts available while applying category rules.
pub struct CategoryContext<'a> {
    pub source: IngressSource,
    pub url: &'a Url,
    pub file_name: Option<&'a str>,
    pub mime_type: Option<&'a str>,
}

/// Evaluates enabled rules by ascending priority; the first complete match wins.
#[must_use]
pub fn select_category(
    rules: &[CategoryRule],
    context: &CategoryContext<'_>,
    default: Option<CategoryId>,
) -> Option<CategoryId> {
    let mut ordered = rules.iter().filter(|rule| rule.enabled).collect::<Vec<_>>();
    ordered.sort_by_key(|rule| rule.priority);
    ordered
        .into_iter()
        .find(|rule| matches_rule(rule, context))
        .map_or(default, |rule| Some(rule.category_id))
}

fn matches_rule(rule: &CategoryRule, context: &CategoryContext<'_>) -> bool {
    rule.source.is_none_or(|source| source == context.source)
        && rule.domain.as_ref().is_none_or(|domain| {
            context
                .url
                .host_str()
                .is_some_and(|host| host.eq_ignore_ascii_case(domain))
        })
        && rule
            .protocol
            .as_ref()
            .is_none_or(|protocol| context.url.scheme().eq_ignore_ascii_case(protocol))
        && rule.extension.as_ref().is_none_or(|extension| {
            context.file_name.is_some_and(|name| {
                name.rsplit_once('.').is_some_and(|(_, current)| {
                    current.eq_ignore_ascii_case(extension.trim_start_matches('.'))
                })
            })
        })
        && rule.mime_type.as_ref().is_none_or(|mime| {
            context
                .mime_type
                .is_some_and(|current| current.eq_ignore_ascii_case(mime))
        })
        && rule.name_regex.as_ref().is_none_or(|pattern| {
            context
                .file_name
                .is_some_and(|name| Regex::new(pattern).is_ok_and(|regex| regex.is_match(name)))
        })
}
