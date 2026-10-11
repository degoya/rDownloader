//! LinkFilter rules (RD-1240-09): which link the LinkGrabber hides, keeps or files away, decided
//! by the first enabled rule in order whose conditions all hold — JDownloader's LinkFilter.
//!
//! Pure, like the category rules beside it: the rules and the facts of one link go in, the rule
//! that decides comes out. `rd-db` applies the answer at intake and when a person re-applies the
//! rules to the list.

use rd_core::{IngressSource, LinkFilterNameSyntax, LinkFilterRule};
use regex::{Regex, RegexBuilder};
use url::Url;

/// Facts available while a link is matched against the filter rules.
pub struct LinkFilterContext<'a> {
    pub source: IngressSource,
    pub url: &'a Url,
    pub file_name: Option<&'a str>,
    /// Size in bytes, once a source or the online check told it.
    pub size: Option<u64>,
}

/// The enabled rules in evaluation order, each name pattern compiled once for a whole batch.
pub struct LinkFilters<'a> {
    ordered: Vec<(&'a LinkFilterRule, Option<Regex>)>,
}

impl<'a> LinkFilters<'a> {
    #[must_use]
    pub fn new(rules: &'a [LinkFilterRule]) -> Self {
        let mut ordered = rules.iter().filter(|rule| rule.enabled).collect::<Vec<_>>();
        ordered.sort_by_key(|rule| rule.position);
        Self {
            ordered: ordered
                .into_iter()
                .map(|rule| {
                    // A pattern that does not compile compiles to nothing, and a rule with a
                    // pattern but nothing compiled never matches -- as with the category rules.
                    let pattern = rule
                        .name_pattern
                        .as_deref()
                        .and_then(|pattern| compile_name_pattern(pattern, rule.name_syntax));
                    (rule, pattern)
                })
                .collect(),
        }
    }

    /// Whether no rule is enabled, so there is nothing to ask.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ordered.is_empty()
    }

    /// The first rule whose conditions all hold for `context`, or `None`.
    #[must_use]
    pub fn decide(&self, context: &LinkFilterContext<'_>) -> Option<&'a LinkFilterRule> {
        self.ordered
            .iter()
            .find(|(rule, pattern)| matches_rule(rule, pattern.as_ref(), context))
            .map(|(rule, _)| *rule)
    }
}

/// The compiled form of a name pattern; `None` for one that does not compile.
///
/// A glob matches the whole name with case ignored, the way a person writes `*.nfo`; a regular
/// expression is searched as written, so `(?i)` and anchors are the writer's choice.
#[must_use]
pub fn compile_name_pattern(pattern: &str, syntax: LinkFilterNameSyntax) -> Option<Regex> {
    match syntax {
        LinkFilterNameSyntax::Glob => RegexBuilder::new(&glob_expression(pattern))
            .case_insensitive(true)
            .build()
            .ok(),
        LinkFilterNameSyntax::Regex => Regex::new(pattern).ok(),
    }
}

/// `*` any run of characters, `?` one character, everything else literally, the whole name.
fn glob_expression(glob: &str) -> String {
    let mut expression = String::with_capacity(glob.len() + 8);
    expression.push('^');
    let mut buffer = [0_u8; 4];
    for character in glob.chars() {
        match character {
            '*' => expression.push_str(".*"),
            '?' => expression.push('.'),
            other => expression.push_str(&regex::escape(other.encode_utf8(&mut buffer))),
        }
    }
    expression.push('$');
    expression
}

fn matches_rule(
    rule: &LinkFilterRule,
    pattern: Option<&Regex>,
    context: &LinkFilterContext<'_>,
) -> bool {
    rule.source.is_none_or(|source| source == context.source)
        && rule.hoster.as_deref().is_none_or(|hoster| {
            context
                .url
                .host_str()
                .is_some_and(|host| host_matches(host, hoster))
        })
        && (rule.extensions.is_empty()
            || context
                .file_name
                .is_some_and(|name| has_extension(name, &rule.extensions)))
        && rule
            .size_min
            .is_none_or(|minimum| context.size.is_some_and(|size| size >= minimum))
        && rule
            .size_max
            .is_none_or(|maximum| context.size.is_some_and(|size| size <= maximum))
        && rule.name_pattern.as_ref().is_none_or(|_| {
            pattern
                .is_some_and(|pattern| context.file_name.is_some_and(|name| pattern.is_match(name)))
        })
}

/// The host itself or one of its subdomains: `rg.example` holds for `www.rg.example`,
/// never for `notrg.example`.
fn host_matches(host: &str, hoster: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let hoster = hoster.trim().trim_end_matches('.').to_ascii_lowercase();
    !hoster.is_empty()
        && (host == hoster
            || host
                .strip_suffix(hoster.as_str())
                .is_some_and(|rest| rest.ends_with('.')))
}

/// Whether the name ends in `.<extension>` for one of them, case ignored; `part1.rar` works as
/// well as `rar`.
fn has_extension(name: &str, extensions: &[String]) -> bool {
    let name = name.to_lowercase();
    extensions.iter().any(|extension| {
        let extension = extension.trim().trim_start_matches('.').to_lowercase();
        !extension.is_empty()
            && name
                .strip_suffix(extension.as_str())
                .is_some_and(|rest| rest.ends_with('.'))
    })
}

#[cfg(test)]
#[path = "link_filters_tests.rs"]
mod tests;
