//! The regex pairs of the package-name rules (RD-1140-05): find → replace, in their order, after
//! the four switches.
//!
//! The `regex` crate runs in linear time and has neither backreferences nor lookaround, so a
//! pattern somebody saved cannot stall the intake; a compiled pattern is also capped in size.
//! Every match of a pattern is replaced, and a replacement may name groups as `$1`, `${1}` or
//! `${name}`. The lists are checked when they are saved; a pattern that still fails to compile
//! here — a hand-edited database — is skipped with a warning rather than stopping the package.

use rd_core::{MAX_PACKAGE_NAME_REGEX_CHARS, MAX_PACKAGE_NAME_REGEX_RULES, PackageNameRegex};
use regex::{Regex, RegexBuilder};

/// Ceiling on a compiled pattern; far above what 200 characters need, far below a problem.
const COMPILED_SIZE_LIMIT: usize = 1 << 20;

/// Compiles one pattern the way the package-name rules and their tester run it.
///
/// # Errors
///
/// The compiler's error for a pattern that is not valid or too large.
pub fn package_name_regex(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(COMPILED_SIZE_LIMIT)
        .build()
}

/// Why a list of regex pairs cannot be saved; each carries a stable code.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PackageNameRegexError {
    #[error("at most {max} regex rules are allowed")]
    TooMany { max: usize },
    /// `index` counts from 1, as the list shows it.
    #[error("rule {index}: pattern and replacement may have at most {max} characters each")]
    TooLong { index: usize, max: usize },
    #[error("rule {index}: {detail}")]
    Invalid { index: usize, detail: String },
}

impl PackageNameRegexError {
    /// The stable code the interface translates.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TooMany { .. } => "settings.package_name_regex_too_many",
            Self::TooLong { .. } => "settings.package_name_regex_too_long",
            Self::Invalid { .. } => "settings.package_name_regex_invalid",
        }
    }
}

/// Checks a list before it is saved: at most ten pairs, each pattern and replacement at most
/// 200 characters, every pattern compiling. A pair with an empty pattern is the caller's to drop.
///
/// # Errors
///
/// The first thing wrong with the list.
pub fn validate_package_name_regex(
    pairs: &[PackageNameRegex],
) -> Result<(), PackageNameRegexError> {
    if pairs.len() > MAX_PACKAGE_NAME_REGEX_RULES {
        return Err(PackageNameRegexError::TooMany {
            max: MAX_PACKAGE_NAME_REGEX_RULES,
        });
    }
    for (position, pair) in pairs.iter().enumerate() {
        let index = position + 1;
        if pair.pattern.chars().count() > MAX_PACKAGE_NAME_REGEX_CHARS
            || pair.replacement.chars().count() > MAX_PACKAGE_NAME_REGEX_CHARS
        {
            return Err(PackageNameRegexError::TooLong {
                index,
                max: MAX_PACKAGE_NAME_REGEX_CHARS,
            });
        }
        if let Err(error) = package_name_regex(&pair.pattern) {
            return Err(PackageNameRegexError::Invalid {
                index,
                detail: error.to_string(),
            });
        }
    }
    Ok(())
}

/// `name` with every pair applied in order. A result that is empty once trimmed leaves the name
/// as it was before the pairs.
pub(crate) fn apply_package_name_regex(name: &str, pairs: &[PackageNameRegex]) -> String {
    if pairs.is_empty() {
        return name.to_owned();
    }
    let mut renamed = name.to_owned();
    for pair in pairs {
        match package_name_regex(&pair.pattern) {
            Ok(regex) => {
                renamed = regex
                    .replace_all(&renamed, pair.replacement.as_str())
                    .into_owned();
            }
            Err(error) => {
                tracing::warn!(pattern = %pair.pattern, %error, "a package-name regex does not compile and is skipped");
            }
        }
    }
    let renamed = renamed.trim();
    if renamed.is_empty() {
        name.to_owned()
    } else {
        renamed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use rd_core::PackageNameRegex;

    use super::{PackageNameRegexError, apply_package_name_regex, validate_package_name_regex};

    fn pair(pattern: &str, replacement: &str) -> PackageNameRegex {
        PackageNameRegex {
            pattern: pattern.to_owned(),
            replacement: replacement.to_owned(),
        }
    }

    #[test]
    fn every_match_is_replaced_and_groups_are_named() {
        let pairs = [
            pair(r"_", "."),
            pair(r"^(?P<title>.+?)\.Update\.v(\d[\d.]*)", "${title} v$2"),
        ];
        assert_eq!(
            apply_package_name_regex("Sintel_Directors_Cut_Update_v1.0.2_EXAMPLE", &pairs),
            "Sintel.Directors.Cut v1.0.2.EXAMPLE"
        );
    }

    #[test]
    fn the_pairs_run_in_their_order() {
        let first = [pair("a", "b"), pair("b", "c")];
        let second = [pair("b", "c"), pair("a", "b")];
        assert_eq!(apply_package_name_regex("ab", &first), "cc");
        assert_eq!(apply_package_name_regex("ab", &second), "bc");
    }

    #[test]
    fn an_empty_result_keeps_the_name() {
        assert_eq!(
            apply_package_name_regex("[1080p]", &[pair(r"\[.*\]", " ")]),
            "[1080p]"
        );
    }

    #[test]
    fn a_pattern_that_does_not_compile_is_skipped() {
        let pairs = [pair("(", "x"), pair("a", "o")];
        assert_eq!(apply_package_name_regex("banana", &pairs), "bonono");
    }

    #[test]
    fn a_list_is_checked_before_it_is_saved() {
        assert_eq!(validate_package_name_regex(&[pair(r"(\d+)", "$1")]), Ok(()));
        let invalid = validate_package_name_regex(&[pair("ok", ""), pair("(", "")]);
        assert!(
            matches!(
                invalid,
                Err(PackageNameRegexError::Invalid { index: 2, .. })
            ),
            "{invalid:?}"
        );
        // Lookaround and backreferences are not part of the engine at all.
        assert!(validate_package_name_regex(&[pair("(?=x)", "")]).is_err());
        assert!(validate_package_name_regex(&[pair(r"(a)\1", "")]).is_err());
        let many: Vec<_> = std::iter::repeat_n(pair("a", ""), 11).collect();
        assert_eq!(
            validate_package_name_regex(&many),
            Err(PackageNameRegexError::TooMany { max: 10 })
        );
        assert_eq!(
            validate_package_name_regex(&[pair(&"a".repeat(201), "")]),
            Err(PackageNameRegexError::TooLong { index: 1, max: 200 })
        );
        assert_eq!(
            validate_package_name_regex(&[pair("a", &"b".repeat(201))])
                .map_err(|error| error.code()),
            Err("settings.package_name_regex_too_long")
        );
    }
}
