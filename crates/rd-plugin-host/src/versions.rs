//! Which installed version of a plugin new work runs on (RD-140-02).
//!
//! Installing never removes an older version, so several can sit side by side. Until 1.4 the
//! highest SemVer always won; now the operator can point a plugin at one version (the active
//! one, e.g. after a rollback) and put a second one under test (the staged one). Both are read
//! from the database once at start: a choice made later takes effect at the next start, the
//! same way installing, removing and switching a plugin off do.
//!
//! The rules, in one place so every consumer applies the same ones:
//!
//! - The **default** version — the one new work runs on — is the chosen active version while it
//!   loaded and is not the staged one; otherwise the newest loaded version that is not staged.
//!   A plugin without a choice therefore behaves exactly as before.
//! - A **staged** version is never the default. Only a job pinned to it explicitly runs it.
//! - Every other loaded version is **retained**: it serves the jobs pinned to it and nothing new.
//!
//! "Loaded" is the point: a withdrawn package, a revoked key or a tampered file never reaches
//! this far, so a withdrawn version can be neither the default nor under test.

use std::collections::HashMap;

/// What the operator chose for one plugin id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionChoice {
    /// The version to run; `None` means the newest installed one.
    pub active: Option<String>,
    /// A version under test, reached only by a job pinned to it.
    pub staged: Option<String>,
}

/// The choices of every plugin that has one, keyed by plugin id.
pub type VersionChoices = HashMap<String, VersionChoice>;

/// Where one loaded version stands. The order is the order consumers see them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VersionRole {
    /// The version new work runs on.
    Default,
    /// Kept for the jobs pinned to it, never chosen for new work.
    Retained,
    /// Under test; reached only through an explicit pin.
    Staged,
}

/// The default version out of the versions of one plugin that actually loaded.
///
/// `None` only when every loaded version is the staged one.
#[must_use]
pub fn default_version<'a>(loaded: &[&'a str], choice: Option<&VersionChoice>) -> Option<&'a str> {
    let staged = choice.and_then(|choice| choice.staged.as_deref());
    if let Some(active) = choice.and_then(|choice| choice.active.as_deref())
        && Some(active) != staged
        && let Some(found) = loaded.iter().copied().find(|version| *version == active)
    {
        return Some(found);
    }
    loaded
        .iter()
        .copied()
        .filter(|version| Some(*version) != staged)
        .max_by(|left, right| compare_versions(left, right))
}

/// The role of `version`, given the plugin's default version and choice.
#[must_use]
pub(crate) fn version_role(
    version: &str,
    default: Option<&str>,
    choice: Option<&VersionChoice>,
) -> VersionRole {
    if default == Some(version) {
        VersionRole::Default
    } else if choice.and_then(|choice| choice.staged.as_deref()) == Some(version) {
        VersionRole::Staged
    } else {
        VersionRole::Retained
    }
}

/// The role of every `(plugin id, version)` pair, computed per id.
#[must_use]
pub(crate) fn roles_by_id(
    pairs: &[(String, String)],
    choices: &VersionChoices,
) -> HashMap<(String, String), VersionRole> {
    let mut by_id: HashMap<&str, Vec<&str>> = HashMap::new();
    for (id, version) in pairs {
        by_id.entry(id.as_str()).or_default().push(version.as_str());
    }
    let mut roles = HashMap::new();
    for (id, versions) in by_id {
        let choice = choices.get(id);
        let default = default_version(&versions, choice);
        for version in versions {
            roles.insert(
                (id.to_owned(), version.to_owned()),
                version_role(version, default, choice),
            );
        }
    }
    roles
}

/// SemVer ascending; a version that does not parse sorts below every one that does.
fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    semver::Version::parse(left)
        .ok()
        .cmp(&semver::Version::parse(right).ok())
}

#[cfg(test)]
mod tests {
    use super::{VersionChoice, VersionRole, default_version, roles_by_id, version_role};

    fn choice(active: Option<&str>, staged: Option<&str>) -> VersionChoice {
        VersionChoice {
            active: active.map(str::to_owned),
            staged: staged.map(str::to_owned),
        }
    }

    #[test]
    fn without_a_choice_the_newest_version_wins_as_before() {
        assert_eq!(
            default_version(&["1.0.0", "1.10.0", "1.2.0"], None),
            Some("1.10.0")
        );
    }

    #[test]
    fn the_chosen_active_version_wins_while_it_is_loaded() {
        let rolled_back = choice(Some("1.0.0"), None);
        assert_eq!(
            default_version(&["1.0.0", "2.0.0"], Some(&rolled_back)),
            Some("1.0.0")
        );
        // Removed or withdrawn: nothing loaded answers to it, so the newest takes over.
        assert_eq!(
            default_version(&["2.0.0", "3.0.0"], Some(&rolled_back)),
            Some("3.0.0")
        );
    }

    #[test]
    fn a_staged_version_is_never_the_default() {
        let staged = choice(None, Some("2.0.0"));
        assert_eq!(
            default_version(&["1.0.0", "2.0.0"], Some(&staged)),
            Some("1.0.0")
        );
        assert_eq!(default_version(&["2.0.0"], Some(&staged)), None);
        // Even when the pointer names it too, which the API refuses to write.
        let both = choice(Some("2.0.0"), Some("2.0.0"));
        assert_eq!(
            default_version(&["1.0.0", "2.0.0"], Some(&both)),
            Some("1.0.0")
        );
    }

    #[test]
    fn every_version_gets_exactly_one_role() {
        let staged = choice(Some("1.0.0"), Some("3.0.0"));
        assert_eq!(
            version_role("1.0.0", Some("1.0.0"), Some(&staged)),
            VersionRole::Default
        );
        assert_eq!(
            version_role("2.0.0", Some("1.0.0"), Some(&staged)),
            VersionRole::Retained
        );
        assert_eq!(
            version_role("3.0.0", Some("1.0.0"), Some(&staged)),
            VersionRole::Staged
        );

        let pairs = [
            ("a".to_owned(), "1.0.0".to_owned()),
            ("a".to_owned(), "3.0.0".to_owned()),
            ("b".to_owned(), "1.0.0".to_owned()),
        ];
        let choices: super::VersionChoices = [("a".to_owned(), choice(None, Some("3.0.0")))].into();
        let roles = roles_by_id(&pairs, &choices);
        assert_eq!(
            roles[&("a".to_owned(), "1.0.0".to_owned())],
            VersionRole::Default
        );
        assert_eq!(
            roles[&("a".to_owned(), "3.0.0".to_owned())],
            VersionRole::Staged
        );
        assert_eq!(
            roles[&("b".to_owned(), "1.0.0".to_owned())],
            VersionRole::Default
        );
    }
}
