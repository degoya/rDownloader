use rd_core::{CategoryId, CategoryRule, CategoryRuleNameTarget, IngressSource};
use regex::Regex;
use url::Url;

/// Facts available while applying category rules.
pub struct CategoryContext<'a> {
    pub source: IngressSource,
    pub url: &'a Url,
    pub file_name: Option<&'a str>,
    /// The name of the package the link is grouped into, for a rule whose `name_regex` targets
    /// the package (RD-1140-02).
    pub package_name: Option<&'a str>,
    pub mime_type: Option<&'a str>,
}

/// Evaluates enabled rules by ascending priority; the first complete match wins.
///
/// For one evaluation; a caller routing many links against the same rules prepares them once
/// with [`CategoryRules`].
#[must_use]
pub fn select_category(
    rules: &[CategoryRule],
    context: &CategoryContext<'_>,
    default: Option<CategoryId>,
) -> Option<CategoryId> {
    CategoryRules::new(rules).select(context, default)
}

/// The enabled rules in evaluation order, each `name_regex` compiled once rather than once per
/// link (audit 1.9.1, INTAKE-10).
pub struct CategoryRules<'a> {
    ordered: Vec<(&'a CategoryRule, Option<Regex>)>,
}

impl<'a> CategoryRules<'a> {
    #[must_use]
    pub fn new(rules: &'a [CategoryRule]) -> Self {
        let mut ordered = rules.iter().filter(|rule| rule.enabled).collect::<Vec<_>>();
        ordered.sort_by_key(|rule| rule.priority);
        Self {
            ordered: ordered
                .into_iter()
                .map(|rule| {
                    // An invalid pattern compiles to nothing, and a rule with a pattern but
                    // nothing compiled never matches -- as when it was compiled per link.
                    let regex = rule
                        .name_regex
                        .as_deref()
                        .and_then(|pattern| Regex::new(pattern).ok());
                    (rule, regex)
                })
                .collect(),
        }
    }

    /// The category of the first rule that matches `context`, else `default`.
    #[must_use]
    pub fn select(
        &self,
        context: &CategoryContext<'_>,
        default: Option<CategoryId>,
    ) -> Option<CategoryId> {
        self.ordered
            .iter()
            .find(|(rule, regex)| matches_rule(rule, regex.as_ref(), context))
            .map_or(default, |(rule, _)| Some(rule.category_id))
    }
}

fn matches_rule(
    rule: &CategoryRule,
    name_regex: Option<&Regex>,
    context: &CategoryContext<'_>,
) -> bool {
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
        && rule
            .name_regex
            .as_ref()
            .is_none_or(|_| name_regex.is_some_and(|regex| names_match(rule, regex, context)))
}

/// Whether `regex` matches the name, or names, the rule's `name_target` points at; an absent
/// name never matches.
fn names_match(rule: &CategoryRule, regex: &Regex, context: &CategoryContext<'_>) -> bool {
    let matches = |name: Option<&str>| name.is_some_and(|name| regex.is_match(name));
    match rule.name_target {
        CategoryRuleNameTarget::File => matches(context.file_name),
        CategoryRuleNameTarget::Package => matches(context.package_name),
        CategoryRuleNameTarget::Either => {
            matches(context.file_name) || matches(context.package_name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CategoryContext, CategoryRules, select_category};
    use rd_core::{
        CategoryId, CategoryRule, CategoryRuleId, CategoryRuleNameTarget, IngressSource,
    };
    use url::Url;

    fn rule(priority: i32, name_regex: &str) -> CategoryRule {
        CategoryRule {
            id: CategoryRuleId::new(),
            name: format!("rule {priority}"),
            priority,
            source: None,
            domain: None,
            protocol: None,
            extension: None,
            mime_type: None,
            name_regex: Some(name_regex.to_owned()),
            name_target: CategoryRuleNameTarget::File,
            category_id: CategoryId::new(),
            enabled: true,
        }
    }

    #[test]
    fn prepared_rules_choose_what_a_single_evaluation_chooses() {
        // An invalid pattern ranks first and never matches; the valid one behind it does.
        let rules = [rule(1, "(unclosed"), rule(2, r"(?i)\.mkv$")];
        let default = Some(CategoryId::new());
        let url: Url = "https://example.test/files".parse().expect("url");
        let prepared = CategoryRules::new(&rules);
        for (file_name, expected) in [
            (Some("Show.S01E01.MKV"), Some(rules[1].category_id)),
            (Some("Show.S01E01.rar"), default),
            (None, default),
        ] {
            let context = CategoryContext {
                source: IngressSource::Manual,
                url: &url,
                file_name,
                package_name: None,
                mime_type: None,
            };
            assert_eq!(
                prepared.select(&context, default),
                expected,
                "{file_name:?}"
            );
            assert_eq!(select_category(&rules, &context, default), expected);
        }
    }

    /// RD-1140-02: the pattern is matched against the name the rule targets -- a hit and a miss
    /// for each target, and the file target (every rule stored before the choice) ignores the
    /// package name entirely.
    #[test]
    fn the_name_pattern_is_matched_against_the_targeted_name() {
        let url: Url = "https://example.test/files".parse().expect("url");
        let default = Some(CategoryId::new());
        let release = "Game.Update.v1.2.0.NSW-GROUP";
        let obfuscated = "a8f3c91d0e.rar";
        for (target, file_name, package_name, matches) in [
            (
                CategoryRuleNameTarget::File,
                Some(release),
                Some(obfuscated),
                true,
            ),
            (
                CategoryRuleNameTarget::File,
                Some(obfuscated),
                Some(release),
                false,
            ),
            (
                CategoryRuleNameTarget::Package,
                Some(obfuscated),
                Some(release),
                true,
            ),
            (
                CategoryRuleNameTarget::Package,
                Some(release),
                Some(obfuscated),
                false,
            ),
            (CategoryRuleNameTarget::Package, Some(release), None, false),
            (
                CategoryRuleNameTarget::Either,
                Some(release),
                Some(obfuscated),
                true,
            ),
            (
                CategoryRuleNameTarget::Either,
                Some(obfuscated),
                Some(release),
                true,
            ),
            (CategoryRuleNameTarget::Either, None, Some(release), true),
            (
                CategoryRuleNameTarget::Either,
                Some(obfuscated),
                Some(obfuscated),
                false,
            ),
            (CategoryRuleNameTarget::Either, None, None, false),
        ] {
            let mut updates = rule(1, "(?i)update.*nsw-");
            updates.name_target = target;
            let context = CategoryContext {
                source: IngressSource::Manual,
                url: &url,
                file_name,
                package_name,
                mime_type: None,
            };
            let expected = if matches {
                Some(updates.category_id)
            } else {
                default
            };
            assert_eq!(
                select_category(std::slice::from_ref(&updates), &context, default),
                expected,
                "{target:?} file {file_name:?} package {package_name:?}"
            );
        }
    }
}
