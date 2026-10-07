//! Package-name rules (RD-1140-05): which clean-ups a new package's name gets.
//!
//! Only the vocabulary lives here: the global switches, a category's override and how the two
//! combine. Applying them to a name is `rd_files::tidy_package_name`, beside the folder-name rules
//! the result goes through afterwards.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The rules of the "Tidy file names" plugin, applied to a package name when the package is
/// created. All off by default: a package keeps the name it was given unless somebody asked.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(default)]
pub struct PackageNameRules {
    /// `Big Buck Bunny` -> `Big.Buck.Bunny`
    pub spaces_to_dots: bool,
    /// `Big..Buck._.Bunny` -> `Big.Buck.Bunny`
    pub collapse_separators: bool,
    /// `[1080p]`, `(x264)` and `{…}` tags are removed, brackets and all
    pub strip_bracket_tags: bool,
    /// `Big.Buck.Bunny` -> `big.buck.bunny`
    pub lowercase: bool,
}

impl PackageNameRules {
    /// Whether any rule is on; with none the name is left exactly as it is.
    #[must_use]
    pub const fn any(self) -> bool {
        self.spaces_to_dots || self.collapse_separators || self.strip_bracket_tags || self.lowercase
    }
}

/// A category's override of the package-name rules; every `None` inherits the global switch.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
#[serde(default)]
pub struct PackageNameRulesOverride {
    pub spaces_to_dots: Option<bool>,
    pub collapse_separators: Option<bool>,
    pub strip_bracket_tags: Option<bool>,
    pub lowercase: Option<bool>,
}

impl PackageNameRulesOverride {
    /// Whether the override sets nothing, so the category inherits every rule.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.spaces_to_dots.is_none()
            && self.collapse_separators.is_none()
            && self.strip_bracket_tags.is_none()
            && self.lowercase.is_none()
    }

    /// The rules in force: each switch this override sets, else the global one.
    #[must_use]
    pub fn resolve(self, global: PackageNameRules) -> PackageNameRules {
        PackageNameRules {
            spaces_to_dots: self.spaces_to_dots.unwrap_or(global.spaces_to_dots),
            collapse_separators: self
                .collapse_separators
                .unwrap_or(global.collapse_separators),
            strip_bracket_tags: self.strip_bracket_tags.unwrap_or(global.strip_bracket_tags),
            lowercase: self.lowercase.unwrap_or(global.lowercase),
        }
    }
}

/// Most find → replace pairs one list may hold.
pub const MAX_PACKAGE_NAME_REGEX_RULES: usize = 10;

/// Longest pattern, and longest replacement, of one pair, in characters.
pub const MAX_PACKAGE_NAME_REGEX_CHARS: usize = 200;

/// One find → replace pair of the package-name regex rules, applied after the four switches.
///
/// `pattern` is a `regex`-crate expression (linear time: no backreferences, no lookaround);
/// every match is replaced by `replacement`, which may name groups as `$1`, `${1}` or `${name}`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq, ToSchema)]
pub struct PackageNameRegex {
    pub pattern: String,
    #[serde(default)]
    pub replacement: String,
}

/// Everything that decides what a new package is called: the switches, then the regex pairs in
/// their order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageNaming {
    pub rules: PackageNameRules,
    pub regex: Vec<PackageNameRegex>,
}

impl PackageNaming {
    /// The naming in force for a category: each switch its override sets, else this one's; its
    /// own regex list when it has one (an empty one included), else this one's.
    #[must_use]
    pub fn for_category(
        &self,
        rules: PackageNameRulesOverride,
        regex: Option<&[PackageNameRegex]>,
    ) -> Self {
        Self {
            rules: rules.resolve(self.rules),
            regex: regex.map_or_else(|| self.regex.clone(), <[PackageNameRegex]>::to_vec),
        }
    }

    /// Whether this naming changes nothing at all.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        !self.rules.any() && self.regex.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{PackageNameRegex, PackageNameRules, PackageNameRulesOverride, PackageNaming};

    #[test]
    fn every_rule_is_off_until_somebody_switches_it_on() {
        assert!(!PackageNameRules::default().any());
        let stored: PackageNameRules = serde_json::from_str("{}").expect("empty object");
        assert_eq!(stored, PackageNameRules::default());
    }

    #[test]
    fn a_category_overrides_only_the_switches_it_sets() {
        let global = PackageNameRules {
            spaces_to_dots: true,
            lowercase: true,
            ..PackageNameRules::default()
        };
        let category = PackageNameRulesOverride {
            lowercase: Some(false),
            strip_bracket_tags: Some(true),
            ..PackageNameRulesOverride::default()
        };
        assert_eq!(
            category.resolve(global),
            PackageNameRules {
                spaces_to_dots: true,
                collapse_separators: false,
                strip_bracket_tags: true,
                lowercase: false,
            }
        );
        assert!(PackageNameRulesOverride::default().is_empty());
        assert_eq!(PackageNameRulesOverride::default().resolve(global), global);
    }

    #[test]
    fn a_category_list_replaces_the_global_one_and_none_inherits_it() {
        let pair = |pattern: &str| PackageNameRegex {
            pattern: pattern.to_owned(),
            replacement: String::new(),
        };
        let global = PackageNaming {
            rules: PackageNameRules::default(),
            regex: vec![pair("a")],
        };
        let inherited = global.for_category(PackageNameRulesOverride::default(), None);
        assert_eq!(inherited.regex, vec![pair("a")]);
        let own = global.for_category(PackageNameRulesOverride::default(), Some(&[pair("b")]));
        assert_eq!(own.regex, vec![pair("b")]);
        let none = global.for_category(PackageNameRulesOverride::default(), Some(&[]));
        assert!(none.is_noop());
        assert!(!global.is_noop());
    }
}
