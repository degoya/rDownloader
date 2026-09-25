//! Plugin-supplied translations shipped inside the signed `.rdplug` archive.
//!
//! A plugin owns every string that is specific to it: its display name and description,
//! its account credential labels, and the failure codes its resolver emits. The core
//! keeps only the generic `plugin.*` catalogue. Because the locale files are covered by
//! the package digest, translations cannot be swapped without breaking the signature.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Per-file size cap; generous for a few hundred short strings.
pub const MAX_LOCALE_BYTES: u64 = 256 * 1024;
/// Upper bound on locale files in one archive.
pub const MAX_LOCALE_FILES: usize = 16;
/// Every localised plugin must at least ship English, the global fallback.
pub const REQUIRED_LOCALE: &str = "en";

/// One `locales/<lang>.json` document.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginLocale {
    /// Localised display name, overriding the manifest's `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Localised description, overriding `[metadata].description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<PluginLocaleAccount>,
    /// Failure-code translations, keyed exactly as the resolver emits them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub codes: BTreeMap<String, String>,
}

/// Credential form labels for this provider's account editor.
///
/// A provider offering a choice of credential modes needs one label and hint per mode: the
/// same field holds a password in one and an API key in the other, and a form that called it
/// both at once would help nobody. The plain `secret_label`/`secret_hint` stay the fallback,
/// and are all a single-mode provider ever needs.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginLocaleAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_label_login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_hint_login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_label_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_hint_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_hint: Option<String>,
}

/// Extracts the language tag from a `locales/<lang>.json` archive member name.
///
/// Returns `None` for any other path, so callers can reject unexpected members.
#[must_use]
pub fn locale_member_language(name: &str) -> Option<&str> {
    let language = name.strip_prefix("locales/")?.strip_suffix(".json")?;
    valid_language(language).then_some(language)
}

/// Whether `language` is a bare two-letter tag; anything else is refused so member
/// names can never escape the `locales/` directory or collide case-insensitively.
#[must_use]
pub fn valid_language(language: &str) -> bool {
    language.len() == 2 && language.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// Parses and validates one locale document for `slug`.
///
/// Every failure code must live in the plugin's own `<slug>.` namespace, so a plugin can
/// never shadow the core's `plugin.*` catalogue or another provider's translations.
pub fn parse_locale(slug: &str, language: &str, bytes: &[u8]) -> Result<PluginLocale> {
    let locale: PluginLocale =
        serde_json::from_slice(bytes).with_context(|| format!("parse locales/{language}.json"))?;
    let prefix = format!("{slug}.");
    for (code, text) in &locale.codes {
        if !code.starts_with(&prefix) {
            bail!("locales/{language}.json code `{code}` must start with `{prefix}`");
        }
        if text.trim().is_empty() {
            bail!("locales/{language}.json code `{code}` has an empty translation");
        }
    }
    for (field, value) in [("name", &locale.name), ("description", &locale.description)] {
        if value.as_ref().is_some_and(|text| text.trim().is_empty()) {
            bail!("locales/{language}.json {field} is empty");
        }
    }
    Ok(locale)
}

/// Validates the whole locale set of one package.
pub fn validate_locales(slug: &str, locales: &[(String, Vec<u8>)]) -> Result<()> {
    if locales.is_empty() {
        return Ok(());
    }
    if locales.len() > MAX_LOCALE_FILES {
        bail!("plugin ships more than {MAX_LOCALE_FILES} locale files");
    }
    let mut seen = std::collections::BTreeSet::new();
    for (language, bytes) in locales {
        if !valid_language(language) {
            bail!("invalid locale language tag `{language}`");
        }
        if bytes.len() as u64 > MAX_LOCALE_BYTES {
            bail!("locales/{language}.json exceeds its size limit");
        }
        if !seen.insert(language.as_str()) {
            bail!("duplicate locale `{language}`");
        }
        parse_locale(slug, language, bytes)?;
    }
    if !seen.contains(REQUIRED_LOCALE) {
        bail!("a localised plugin must ship locales/{REQUIRED_LOCALE}.json");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(value: &str) -> Vec<u8> {
        value.as_bytes().to_vec()
    }

    #[test]
    fn member_names_map_to_language_tags() {
        assert_eq!(locale_member_language("locales/de.json"), Some("de"));
        assert_eq!(locale_member_language("locales/en.json"), Some("en"));
        assert_eq!(locale_member_language("locales/EN.json"), None);
        assert_eq!(locale_member_language("locales/deu.json"), None);
        assert_eq!(locale_member_language("locales/../evil.json"), None);
        assert_eq!(locale_member_language("component.wasm"), None);
    }

    #[test]
    fn codes_must_use_the_provider_namespace() {
        let good = bytes(r#"{"codes":{"fixture.login_failed":"Login failed"}}"#);
        assert!(parse_locale("fixture", "en", &good).is_ok());

        let core = bytes(r#"{"codes":{"plugin.timeout":"hijacked"}}"#);
        assert!(parse_locale("fixture", "en", &core).is_err());

        let foreign = bytes(r#"{"codes":{"rapidgator.premium_required":"nope"}}"#);
        assert!(parse_locale("fixture", "en", &foreign).is_err());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let extra = bytes(r#"{"unexpected":"value"}"#);
        assert!(parse_locale("fixture", "en", &extra).is_err());
    }

    #[test]
    fn english_is_required_when_locales_are_present() {
        let de = vec![("de".to_owned(), bytes(r#"{"name":"Beispiel"}"#))];
        assert!(validate_locales("fixture", &de).is_err());

        let both = vec![
            ("de".to_owned(), bytes(r#"{"name":"Beispiel"}"#)),
            ("en".to_owned(), bytes(r#"{"name":"Example"}"#)),
        ];
        assert!(validate_locales("fixture", &both).is_ok());

        assert!(validate_locales("fixture", &[]).is_ok());
    }

    #[test]
    fn account_labels_round_trip() {
        // Non-ASCII text is what these files carry in practice, so keep one in the fixture
        // (Rust sources stay English, hence French rather than German).
        let raw = bytes(
            r#"{"account":{"secret_label":"Cl\u00e9 API","secret_hint":"Depuis votre compte"}}"#,
        );
        let locale = parse_locale("fixture", "fr", &raw).expect("parse");
        let account = locale.account.expect("account");
        assert_eq!(account.secret_label.as_deref(), Some("Cl\u{e9} API"));
        assert_eq!(account.secret_hint.as_deref(), Some("Depuis votre compte"));
    }
}
