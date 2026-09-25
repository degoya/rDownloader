//! The environment the external archive tools are started with.
//!
//! RD-107-11: `unrar` converts its `-p` argument from bytes to `wchar_t` through the process
//! locale. A service started without `LANG` runs under `C`/`POSIX`, where a UTF-8 password turns
//! into different characters than in the user's terminal — the same bytes then derive a different
//! key and `unrar` reports "Incorrect password" for a password that is correct. Measured:
//! `LC_ALL=C.UTF-8` succeeds, `LC_ALL=C` exits 11, and so does an empty environment. Clearing the
//! environment therefore makes it worse; the locale has to be set on purpose.

/// Used when nothing inherited names a UTF-8 charset. Built into glibc and always present.
const FALLBACK_LOCALE: &str = "C.UTF-8";

/// Sets the character encoding the tool must use for its arguments.
///
/// An inherited UTF-8 locale is kept (a user who runs `de_DE.UTF-8` keeps their collation);
/// anything else is replaced, because anything else mangles non-ASCII passwords.
pub(crate) fn apply_tool_environment(command: &mut tokio::process::Command) {
    let locale = utf8_locale(|name| std::env::var(name).ok());
    // LC_ALL outranks both of the others, so LC_CTYPE cannot reintroduce a non-UTF-8 charset.
    command.env("LC_ALL", &locale).env("LANG", locale);
}

/// The locale value to run the tool under, given a lookup into the inherited environment.
///
/// Split out from the spawn so the rule can be tested without a process and without touching
/// this process's own environment.
pub(crate) fn utf8_locale(lookup: impl Fn(&str) -> Option<String>) -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .filter_map(&lookup)
        .find(|value| is_utf8_locale(value))
        .unwrap_or_else(|| FALLBACK_LOCALE.to_owned())
}

/// Whether a locale name such as `de_DE.UTF-8` or `C.utf8@euro` names the UTF-8 charset.
fn is_utf8_locale(value: &str) -> bool {
    let Some((_, charset)) = value.rsplit_once('.') else {
        return false;
    };
    let charset = charset.split('@').next().unwrap_or(charset);
    let compact: String = charset
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    compact.eq_ignore_ascii_case("utf8")
}

#[cfg(test)]
mod tests {
    use super::{FALLBACK_LOCALE, utf8_locale};

    fn from(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn a_service_without_a_locale_gets_the_utf8_fallback() {
        assert_eq!(utf8_locale(|_| None), FALLBACK_LOCALE);
        assert_eq!(utf8_locale(from(&[("LANG", "C")])), FALLBACK_LOCALE);
        assert_eq!(utf8_locale(from(&[("LC_ALL", "POSIX")])), FALLBACK_LOCALE);
        assert_eq!(
            utf8_locale(from(&[("LANG", "de_DE.ISO-8859-1")])),
            FALLBACK_LOCALE
        );
    }

    #[test]
    fn an_inherited_utf8_locale_is_kept_in_precedence_order() {
        assert_eq!(
            utf8_locale(from(&[("LC_ALL", "de_DE.UTF-8"), ("LANG", "en_US.UTF-8")])),
            "de_DE.UTF-8"
        );
        assert_eq!(
            utf8_locale(from(&[("LC_ALL", "C"), ("LC_CTYPE", "en_US.utf8")])),
            "en_US.utf8"
        );
        assert_eq!(
            utf8_locale(from(&[("LANG", "fr_FR.UTF8@euro")])),
            "fr_FR.UTF8@euro"
        );
    }
}
