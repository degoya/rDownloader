//! The language the agent's intake toast speaks (RD-1270-05): the desktop's, when it is one of
//! the four the interface ships, English otherwise.
//!
//! Read once, from what the desktop states: the POSIX variables first -- the language of a Linux
//! or macOS session, and the way to choose one on any system -- then, only when none of them
//! names one of ours, the system's setting: on Windows, where the variables are normally unset,
//! the display language the user chose and the regional format; on macOS, where an agent started
//! by launchd sees no variable either, the preferred languages and the region (RD-1270-19).

use std::sync::OnceLock;

/// The languages the toast is written in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lang {
    En,
    De,
    Es,
    Fr,
}

impl Lang {
    /// Every language, in the order of [`Lang::index`].
    pub(crate) const ALL: [Self; 4] = [Self::En, Self::De, Self::Es, Self::Fr];

    pub(crate) fn index(self) -> usize {
        match self {
            Self::En => 0,
            Self::De => 1,
            Self::Es => 2,
            Self::Fr => 3,
        }
    }

    /// The language a locale tag names -- `de_DE.UTF-8`, `fr-FR`, `es` -- when it is one of
    /// ours; `None` for any other and for `C` or `POSIX`.
    pub(crate) fn of_tag(tag: &str) -> Option<Self> {
        let primary = tag.trim().split(['_', '-', '.', '@']).next()?;
        match primary.to_ascii_lowercase().as_str() {
            "en" => Some(Self::En),
            "de" => Some(Self::De),
            "es" => Some(Self::Es),
            "fr" => Some(Self::Fr),
            _ => None,
        }
    }

    /// The first of `tags` that names one of ours.
    pub(crate) fn find<I, S>(tags: I) -> Option<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        tags.into_iter().find_map(|tag| Self::of_tag(tag.as_ref()))
    }
}

/// The desktop's language, read on first use.
pub(crate) fn current() -> Lang {
    static CURRENT: OnceLock<Lang> = OnceLock::new();
    *CURRENT.get_or_init(|| choose(&variable_tags(|name| std::env::var(name).ok()), system_tags))
}

/// The variables before the system: `system` is asked only when no variable names one of ours,
/// and English is the answer when neither does.
fn choose(variables: &[String], system: impl FnOnce() -> Vec<String>) -> Lang {
    Lang::find(variables)
        .or_else(|| Lang::find(system()))
        .unwrap_or(Lang::En)
}

/// The locale tags the POSIX variables state, in the order gettext reads them; `LANGUAGE` may
/// list several, separated by colons.
fn variable_tags(lookup: impl Fn(&str) -> Option<String>) -> Vec<String> {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(lookup)
        .flat_map(|value| value.split(':').map(str::to_owned).collect::<Vec<_>>())
        .collect()
}

/// The display languages the user chose, the system's, and the regional format, from the
/// user's registry hive; whatever of it cannot be read is left out.
#[cfg(windows)]
fn system_tags() -> Vec<String> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    let user = RegKey::predef(HKEY_CURRENT_USER);
    let mut tags = Vec::new();
    for (path, value) in [
        (r"Control Panel\Desktop", "PreferredUILanguages"),
        (
            r"Control Panel\Desktop\MuiCached",
            "MachinePreferredUILanguages",
        ),
    ] {
        if let Ok(list) = user
            .open_subkey(path)
            .and_then(|key| key.get_value::<Vec<String>, _>(value))
        {
            tags.extend(list);
        }
    }
    if let Ok(name) = user
        .open_subkey(r"Control Panel\International")
        .and_then(|key| key.get_value::<String, _>("LocaleName"))
    {
        tags.push(name);
    }
    tags
}

/// The languages the user prefers (`AppleLanguages`, e.g. `de-DE`), then the region's locale
/// (`AppleLocale`, e.g. `de_DE`), through Foundation's safe `NSLocale` bindings -- the user's
/// defaults, which launchd does not hide the way it hides the POSIX variables.
#[cfg(target_os = "macos")]
fn system_tags() -> Vec<String> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::NSLocale;

    // A pool of its own: the first toast may be raised on a thread that drains none.
    autoreleasepool(|_| {
        let mut tags: Vec<String> = NSLocale::preferredLanguages()
            .to_vec()
            .iter()
            .map(|tag| tag.to_string())
            .collect();
        tags.push(NSLocale::currentLocale().localeIdentifier().to_string());
        tags
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_tags() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{Lang, choose, variable_tags};

    #[test]
    fn a_tag_is_read_by_its_language_whatever_follows_it() {
        assert_eq!(Lang::of_tag("de_DE.UTF-8"), Some(Lang::De));
        assert_eq!(Lang::of_tag("fr-FR"), Some(Lang::Fr));
        assert_eq!(Lang::of_tag("es"), Some(Lang::Es));
        assert_eq!(Lang::of_tag("EN_us"), Some(Lang::En));
        assert_eq!(Lang::of_tag("de@euro"), Some(Lang::De));
        for other in ["C", "POSIX", "C.UTF-8", "it_IT.UTF-8", ""] {
            assert_eq!(Lang::of_tag(other), None, "{other}");
        }
    }

    #[test]
    fn the_first_language_of_ours_wins() {
        assert_eq!(Lang::find(["C.UTF-8", "it_IT", "de_AT"]), Some(Lang::De));
        assert_eq!(Lang::find(["fr-CA", "de-DE"]), Some(Lang::Fr));
        assert_eq!(Lang::find(["it_IT", "ja_JP"]), None);
        assert_eq!(Lang::find(Vec::<String>::new()), None);
    }

    fn tags(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn a_variable_naming_one_of_ours_wins_and_the_system_is_not_asked() {
        let chosen = choose(&tags(&["fr_FR.UTF-8"]), || {
            panic!("the system is asked although a variable names a language of ours")
        });
        assert_eq!(chosen, Lang::Fr);
    }

    #[test]
    fn without_a_variable_of_ours_the_system_decides() {
        // launchd sets nothing; `C` or a language we do not ship tells nothing either.
        for variables in [tags(&[]), tags(&["C.UTF-8"]), tags(&["it_IT.UTF-8", "C"])] {
            assert_eq!(
                choose(&variables, || tags(&["it-IT", "de-DE", "en-US", "de_DE"])),
                Lang::De,
                "{variables:?}"
            );
        }
        assert_eq!(choose(&[], || tags(&["es-419", "es_MX"])), Lang::Es);
    }

    #[test]
    fn a_language_neither_side_knows_is_english() {
        assert_eq!(
            choose(&tags(&["it_IT"]), || tags(&["ja-JP", "ja_JP"])),
            Lang::En
        );
        assert_eq!(choose(&[], Vec::new), Lang::En);
    }

    #[test]
    fn the_variables_are_read_in_gettexts_order_and_language_lists_split() {
        let read = variable_tags(|name| match name {
            "LANG" => Some("de_DE.UTF-8".to_owned()),
            "LANGUAGE" => Some("fr:es".to_owned()),
            _ => None,
        });
        assert_eq!(read, tags(&["fr", "es", "de_DE.UTF-8"]));
        assert!(variable_tags(|_| None).is_empty());
    }

    #[test]
    fn every_language_has_its_own_slot() {
        for (index, lang) in Lang::ALL.into_iter().enumerate() {
            assert_eq!(lang.index(), index, "{lang:?}");
        }
    }
}
