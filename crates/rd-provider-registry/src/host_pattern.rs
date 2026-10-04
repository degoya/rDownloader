//! The one reading of a host allowlist entry (RD-191-06, PLUG-17).
//!
//! Six places matched `*` patterns, with three meanings between them; one stripped only the
//! `*`, so `*foo.com` admitted `evilfoo.com`. They all ask this now. It lives here, the lowest
//! crate that needs it, and `rd-core` re-exports it for the crates above — like [`host_key`],
//! the one form of a host name (INTAKE-11).

/// The form of a host name every comparison uses: lower case, without a trailing dot and
/// without a leading `www.`.
///
/// Nine places wrote "lowercase and strip `www.`" inline before this, with small differences
/// (audit 1.9.1, INTAKE-11); a host block, a limit scope and a provider match then disagreed
/// about whether `WWW.Example.com.` was `example.com`.
#[must_use]
pub fn host_key(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    match host.strip_prefix("www.") {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => host,
    }
}

/// Whether a `*.suffix` entry covers the suffix itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WildcardApex {
    /// Sub-domains only — a sandbox allowlist: `*.example.org` reaches `cdn.example.org`,
    /// never `example.org`, which the manifest has to name on its own.
    Excluded,
    /// The suffix and every name below it — a link claim or a site rule, where
    /// `*.example.org` stands for the site.
    Included,
}

/// Whether `host` falls under the allowlist entry `pattern`.
///
/// An entry is a host, matched exactly, or `*.` and a suffix, matched at a label boundary, so
/// `*.foo.com` never covers `evilfoo.com`. A `*` anywhere else makes no pattern and matches
/// nothing; the catch-all `*` on its own is the caller's to allow, because only the caller
/// knows whether it may stand in its list. Both sides are compared as given: hosts are
/// lowercase by the time they get here.
#[must_use]
pub fn host_pattern_matches(pattern: &str, host: &str, apex: WildcardApex) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) if !suffix.is_empty() && !suffix.contains('*') => {
            (apex == WildcardApex::Included && host == suffix)
                || host
                    .strip_suffix(suffix)
                    .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
        }
        Some(_) => false,
        None => !pattern.contains('*') && host == pattern,
    }
}

#[cfg(test)]
mod tests {
    use super::{WildcardApex, host_pattern_matches};

    #[test]
    fn a_wildcard_needs_its_dot_and_a_label_boundary() {
        for apex in [WildcardApex::Excluded, WildcardApex::Included] {
            assert!(host_pattern_matches("*.foo.com", "cdn.foo.com", apex));
            assert!(host_pattern_matches("*.foo.com", "a.b.foo.com", apex));
            assert!(!host_pattern_matches("*.foo.com", "evilfoo.com", apex));
            // A star without its dot is no pattern at all.
            assert!(!host_pattern_matches("*foo.com", "evilfoo.com", apex));
            assert!(!host_pattern_matches("*foo.com", "foo.com", apex));
            assert!(!host_pattern_matches("cdn.*.com", "cdn.foo.com", apex));
            assert!(!host_pattern_matches("*.", "foo.com", apex));
            assert!(!host_pattern_matches("*", "foo.com", apex));
            // A host is equal or nothing.
            assert!(host_pattern_matches("foo.com", "foo.com", apex));
            assert!(!host_pattern_matches("foo.com", "www.foo.com", apex));
        }
    }

    #[test]
    fn the_apex_is_covered_only_when_the_caller_says_so() {
        assert!(!host_pattern_matches(
            "*.foo.com",
            "foo.com",
            WildcardApex::Excluded
        ));
        assert!(host_pattern_matches(
            "*.foo.com",
            "foo.com",
            WildcardApex::Included
        ));
        // An empty first label is not a sub-domain.
        assert!(!host_pattern_matches(
            "*.foo.com",
            ".foo.com",
            WildcardApex::Included
        ));
    }
}
