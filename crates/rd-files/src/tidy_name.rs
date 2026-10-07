//! The package-name rules (RD-1140-05): the clean-ups of the "Tidy file names" plugin, applied
//! to a package name when the package is created.
//!
//! The plugin (`plugins/rename-postprocess/src/rules.rs`) renames files inside a package folder
//! and is a WebAssembly component the host cannot link, so the rules are kept here a second time
//! with the same semantics; the tests below are the plugin's cases. Two differences, both because
//! a package name is not a file name: the whole name is tidied (there is no extension to keep),
//! and the result is trimmed of whitespace at both ends, as every package name is. The folder
//! name still comes from [`crate::package_directory`], which sanitises the tidied name.
//!
//! The regex pairs that follow the switches are `tidy_regex.rs`.

use rd_core::{PackageNameRules, PackageNaming};

fn strip_bracket_tags(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0_u32;
    for character in name.chars() {
        match character {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(character),
            _ => {}
        }
    }
    out
}

fn collapse_separators(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut previous_separator = false;
    for character in name.chars() {
        let separator = matches!(character, '.' | '_' | '-' | ' ');
        if separator {
            if !previous_separator {
                out.push(if character == ' ' { ' ' } else { '.' });
            }
        } else {
            out.push(character);
        }
        previous_separator = separator;
    }
    out.trim_matches(['.', ' ', '_', '-']).to_owned()
}

/// `name` as a new package with this naming is called: the switches, then the regex pairs.
///
/// A step whose result would be empty leaves the name as that step found it.
#[must_use]
pub fn tidy_package_name(name: &str, naming: &PackageNaming) -> String {
    crate::tidy_regex::apply_package_name_regex(&apply_switches(name, naming.rules), &naming.regex)
}

/// `name` with the switched-on rules applied, in the plugin's order: bracket tags, spaces,
/// separators, case.
///
/// A name the rules would empty — `[1080p]` with the tags stripped — is returned unchanged, and
/// so is every name when no rule is on.
fn apply_switches(name: &str, rules: PackageNameRules) -> String {
    if !rules.any() {
        return name.to_owned();
    }
    // A leading dot is held back so the separator rules cannot trim it away, as in the plugin.
    let (hidden, body) = match name.strip_prefix('.') {
        Some(rest) => (".", rest),
        None => ("", name),
    };
    let mut tidied = body.to_owned();
    if rules.strip_bracket_tags {
        tidied = strip_bracket_tags(&tidied);
    }
    if rules.spaces_to_dots {
        tidied = tidied.replace(' ', ".");
    }
    if rules.collapse_separators {
        tidied = collapse_separators(&tidied);
    }
    if rules.lowercase {
        tidied = tidied.to_lowercase();
    }
    let tidied = tidied.trim();
    if tidied.is_empty() {
        return name.to_owned();
    }
    format!("{hidden}{tidied}")
}

#[cfg(test)]
mod tests {
    use rd_core::PackageNameRules;

    use super::{apply_switches, tidy_package_name};

    /// The plugin's default: the two rules it switches on.
    const PLUGIN_DEFAULT: PackageNameRules = PackageNameRules {
        spaces_to_dots: true,
        collapse_separators: true,
        strip_bracket_tags: false,
        lowercase: false,
    };

    const NONE: PackageNameRules = PackageNameRules {
        spaces_to_dots: false,
        collapse_separators: false,
        strip_bracket_tags: false,
        lowercase: false,
    };

    /// The plugin's test cases (`plugins/rename-postprocess/src/rules.rs`), without the
    /// extension a file name has and a package name does not.
    #[test]
    fn the_plugin_cases_give_the_plugin_results() {
        let strip = PackageNameRules {
            strip_bracket_tags: true,
            ..PLUGIN_DEFAULT
        };
        let lower = PackageNameRules {
            lowercase: true,
            ..PLUGIN_DEFAULT
        };
        for (name, rules, expected) in [
            ("Big Buck Bunny", PLUGIN_DEFAULT, "Big.Buck.Bunny"),
            ("Big.Buck.Bunny", PLUGIN_DEFAULT, "Big.Buck.Bunny"),
            ("Big..Buck_-_Bunny", PLUGIN_DEFAULT, "Big.Buck.Bunny"),
            (
                "Big Buck Bunny [1080p] (x264)",
                PLUGIN_DEFAULT,
                "Big.Buck.Bunny.[1080p].(x264)",
            ),
            ("Big Buck Bunny [1080p] (x264)", strip, "Big.Buck.Bunny"),
            ("Big Buck Bunny", lower, "big.buck.bunny"),
            (".hidden file", PLUGIN_DEFAULT, ".hidden.file"),
            ("[1080p]", strip, "[1080p]"),
        ] {
            assert_eq!(apply_switches(name, rules), expected, "{name}");
        }
    }

    #[test]
    fn each_rule_works_on_its_own() {
        let only = |rule: fn(&mut PackageNameRules)| {
            let mut rules = NONE;
            rule(&mut rules);
            rules
        };
        let dots = only(|rules| rules.spaces_to_dots = true);
        let collapse = only(|rules| rules.collapse_separators = true);
        let strip = only(|rules| rules.strip_bracket_tags = true);
        let lower = only(|rules| rules.lowercase = true);
        assert_eq!(apply_switches("Big  Buck Bunny", dots), "Big..Buck.Bunny");
        assert_eq!(
            apply_switches("-Big  Buck__Bunny.", collapse),
            "Big Buck.Bunny"
        );
        assert_eq!(
            apply_switches("Big Buck Bunny [1080p] {x}", strip),
            "Big Buck Bunny"
        );
        assert_eq!(apply_switches("Big Buck BUNNY", lower), "big buck bunny");
    }

    #[test]
    fn the_settings_example_with_every_rule_on() {
        let all = PackageNameRules {
            spaces_to_dots: true,
            collapse_separators: true,
            strip_bracket_tags: true,
            lowercase: true,
        };
        assert_eq!(
            apply_switches("Big Buck Bunny [1080p]", all),
            "big.buck.bunny"
        );
        // Dots without collapsing: the space before the stripped tag stays a dot of its own.
        let dots_and_tags = PackageNameRules {
            spaces_to_dots: true,
            strip_bracket_tags: true,
            ..NONE
        };
        assert_eq!(
            apply_switches("Big Buck Bunny [1080p]", dots_and_tags),
            "Big.Buck.Bunny."
        );
    }

    #[test]
    fn no_rule_leaves_the_name_exactly_as_it_is() {
        assert_eq!(apply_switches(" Big Buck Bunny ", NONE), " Big Buck Bunny ");
    }

    #[test]
    fn a_name_that_would_become_empty_stays_unchanged() {
        let everything = PackageNameRules {
            spaces_to_dots: true,
            collapse_separators: true,
            strip_bracket_tags: true,
            lowercase: true,
        };
        for name in ["[1080p]", "(x264) [1080p]", "...", " - _ "] {
            assert_eq!(apply_switches(name, everything), name);
        }
    }

    #[test]
    fn the_regex_pairs_run_after_the_switches() {
        let naming = rd_core::PackageNaming {
            rules: PackageNameRules {
                spaces_to_dots: true,
                ..NONE
            },
            regex: vec![rd_core::PackageNameRegex {
                pattern: r"\.\[1080p\]$".to_owned(),
                replacement: String::new(),
            }],
        };
        assert_eq!(
            tidy_package_name("Big Buck Bunny [1080p]", &naming),
            "Big.Buck.Bunny"
        );
    }
}
