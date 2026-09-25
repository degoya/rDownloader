//! The small lexical rules the format is built on: what counts as a host, an identifier, a
//! variable name and a well-formed template.
//!
//! Hand-written rather than regular expressions so the crate does not compile a pattern to
//! check whether another pattern's neighbour is a host name.

/// Longest identifier (`id`) a rule may carry.
pub const MAX_ID_LENGTH: usize = 64;
/// Longest group name a rule may carry.
pub const MAX_GROUP_LENGTH: usize = 32;
/// Longest variable name a step may write.
pub const MAX_VARIABLE_LENGTH: usize = 32;

/// Lowercase kebab-case: `[a-z0-9]` then `[a-z0-9-]*`, no trailing hyphen, at most `max`.
#[must_use]
pub fn is_slug(text: &str, max: usize) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= max
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-'
}

/// A variable name: `[a-z][a-z0-9_]*`, at most [`MAX_VARIABLE_LENGTH`].
#[must_use]
pub fn is_variable(text: &str) -> bool {
    let bytes = text.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_VARIABLE_LENGTH
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// A concrete host name: at least two DNS labels, lowercase ASCII, no scheme, port or path.
///
/// Internationalised names are written in their punycode form, which is what the address bar
/// and `url::Url` hand over anyway.
#[must_use]
pub fn is_host(text: &str) -> bool {
    text.len() <= 253 && text.matches('.').count() >= 1 && text.split('.').all(is_label)
}

/// A host pattern as `match.hosts` carries it: a concrete host, or `*.` before one.
#[must_use]
pub fn is_host_pattern(text: &str) -> bool {
    match text.strip_prefix("*.") {
        Some(rest) => is_host(rest),
        None => is_host(text),
    }
}

/// Whether `host` falls under `pattern`: equal for a concrete pattern; for `*.example.org`
/// the apex itself and every name below it.
#[must_use]
pub fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(apex) => {
            host == apex
                || host
                    .strip_suffix(apex)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        }
        None => host == pattern,
    }
}

fn is_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-'
}

/// The variable names a template references through `${name}`, or `None` when a placeholder
/// is unterminated or names something that is not a variable.
#[must_use]
pub fn template_variables(template: &str) -> Option<Vec<&str>> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let end = after.find('}')?;
        let name = &after[..end];
        if !is_variable(name) {
            return None;
        }
        names.push(name);
        rest = &after[end + 1..];
    }
    Some(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_is_lowercase_kebab_case_within_its_length() {
        assert!(is_slug("scnlog", MAX_ID_LENGTH));
        assert!(is_slug("scene-rls-2", MAX_ID_LENGTH));
        assert!(!is_slug("", MAX_ID_LENGTH));
        assert!(!is_slug("-lead", MAX_ID_LENGTH));
        assert!(!is_slug("trail-", MAX_ID_LENGTH));
        assert!(!is_slug("Upper", MAX_ID_LENGTH));
        assert!(!is_slug("under_score", MAX_ID_LENGTH));
        assert!(!is_slug(&"a".repeat(MAX_ID_LENGTH + 1), MAX_ID_LENGTH));
    }

    #[test]
    fn a_variable_starts_with_a_letter_and_allows_underscores() {
        assert!(is_variable("page"));
        assert!(is_variable("links_2"));
        assert!(!is_variable("2links"));
        assert!(!is_variable("Page"));
        assert!(!is_variable("with-dash"));
        assert!(!is_variable(""));
    }

    #[test]
    fn a_host_needs_two_labels_and_nothing_but_lowercase_dns_characters() {
        assert!(is_host("scnlog.me"));
        assert!(is_host("xn--bcher-kva.example"));
        assert!(!is_host("localhost"));
        assert!(!is_host("Scnlog.me"));
        assert!(!is_host("https://scnlog.me"));
        assert!(!is_host("scnlog.me/path"));
        assert!(!is_host("scnlog.me:8080"));
        assert!(!is_host("scnlog..me"));
        assert!(!is_host("-a.me"));
    }

    #[test]
    fn a_wildcard_covers_the_apex_and_everything_below_it() {
        assert!(is_host_pattern("*.pastebin.com"));
        assert!(!is_host_pattern("*.com"));
        assert!(!is_host_pattern("*pastebin.com"));
        assert!(host_matches("*.pastebin.com", "pastebin.com"));
        assert!(host_matches("*.pastebin.com", "www.pastebin.com"));
        assert!(host_matches("*.pastebin.com", "a.b.pastebin.com"));
        assert!(!host_matches("*.pastebin.com", "notpastebin.com"));
        assert!(host_matches("scnlog.me", "scnlog.me"));
        assert!(!host_matches("scnlog.me", "www.scnlog.me"));
    }

    #[test]
    fn a_template_yields_its_variables_or_nothing_when_malformed() {
        assert_eq!(template_variables("plain"), Some(Vec::new()));
        assert_eq!(
            template_variables("${base}/dl/${id}"),
            Some(vec!["base", "id"])
        );
        assert_eq!(template_variables("${open"), None);
        assert_eq!(template_variables("${Bad-Name}"), None);
        assert_eq!(template_variables("${}"), None);
    }
}
