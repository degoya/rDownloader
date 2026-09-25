//! What an indexer says about a hit, kept safely (RD-101-17).
//!
//! [`crate::parse_feed`] already retains every `<newznab:attr>` / `<torznab:attr>` pair, and
//! deliberately so: indexers disagree about which attributes they emit, and an enum would
//! only mean discarding the rest. This module is the gate between that raw map and our
//! database, because three of those values are not ordinary data:
//!
//! * A value can carry the subscription's own API key — Torznab indexers like to put the
//!   passkey inside `magneturl` or `nfo`. Storing it unredacted would defeat the point of
//!   holding the key encrypted in the vault.
//! * `coverurl` and `backdropcoverurl` end up in an `<img src>` in the browser, so anything
//!   that is not a plain `http`/`https` address is dropped rather than rendered.
//! * `password` is a *flag* in the Newznab specification (`0` no, `1` rar pass, `2` inner
//!   archive), not a secret. An indexer that puts a real password there instead is the only
//!   case where one can be taken, and the two must never be confused: a flag stored as a
//!   password would be tried first by the extractor and fail every archive.

use std::collections::BTreeMap;

use rd_core::redact_text;

/// Most attributes an indexer may keep. Newznab defines roughly thirty; the cap is a guard
/// against a hostile feed, not a limit anyone legitimate runs into.
pub const MAX_FIELDS: usize = 40;

/// Longest single value. `imdbplot` is regularly longer than the 512 characters an enricher
/// field is capped at, so this is deliberately twice that.
pub const MAX_VALUE: usize = 1024;

/// Budget for one item's attributes together, so a feed cannot inflate a row without bound.
pub const MAX_TOTAL: usize = 8192;

/// Attribute names whose value is a credential rather than a description.
///
/// Dropped whole rather than redacted: unlike a URL, there is no structure worth keeping.
const SECRET_NAMES: &[&str] = &[
    "apikey", "api_key", "auth", "passkey", "rsstoken", "secret", "token",
];

/// Attribute names whose value is rendered as an image address.
const IMAGE_NAMES: &[&str] = &["coverurl", "backdropcoverurl"];

/// The Newznab attribute that is a flag in the specification and a secret in the wild.
const PASSWORD: &str = "password";

/// What survived the gate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetainedAttributes {
    /// Safe to store and to serialize to a client.
    pub attributes: BTreeMap<String, String>,
    /// A real archive password, when the indexer supplied one instead of the flag. Never
    /// serialized; it travels to the package so the extractor can try it first.
    pub password: Option<String>,
}

/// Filters an indexer's raw attribute map into what may be stored.
///
/// `size_bytes` is the size parsed from `<enclosure length>`, used only when the feed did not
/// send a `size` attribute — so there is exactly one place a size is read from later.
#[must_use]
pub fn retain(raw: &BTreeMap<String, String>, size_bytes: Option<u64>) -> RetainedAttributes {
    let mut out = RetainedAttributes::default();
    let mut budget = MAX_TOTAL;

    for (name, value) in raw {
        if out.attributes.len() >= MAX_FIELDS {
            break;
        }
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || SECRET_NAMES.contains(&name.as_str()) {
            continue;
        }

        if name == PASSWORD {
            let Some((flag, secret)) = password_of(value) else {
                continue;
            };
            out.password = secret;
            insert_within(&mut out.attributes, &mut budget, name, flag);
            continue;
        }

        let value = truncate(&redact_text(value.trim()), MAX_VALUE);
        if value.is_empty() {
            continue;
        }
        if IMAGE_NAMES.contains(&name.as_str()) && !is_displayable_image(&value) {
            continue;
        }
        insert_within(&mut out.attributes, &mut budget, name, value);
    }

    // Only after the loop: an explicit `size` attribute always wins over the enclosure.
    if !out.attributes.contains_key("size")
        && let Some(size) = size_bytes.filter(|size| *size > 0)
        && out.attributes.len() < MAX_FIELDS
    {
        insert_within(
            &mut out.attributes,
            &mut budget,
            "size".to_owned(),
            size.to_string(),
        );
    }
    out
}

/// Splits a `password` attribute into the flag to show and the secret to keep, if any.
///
/// `None` means "say nothing": either the attribute was empty, or it said `0`, which is the
/// same as no attribute at all and would only add a column of zeroes to the UI.
fn password_of(value: &str) -> Option<(String, Option<String>)> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    match value.parse::<u32>() {
        // The specification's flag. `0` is "not protected", which is not worth storing.
        Ok(0) => None,
        Ok(_) => Some((value.to_owned(), None)),
        // Not a number, so it is not the flag — an indexer that writes anything else here is
        // announcing the archive password itself.
        Err(_) => Some(("1".to_owned(), Some(truncate(value, MAX_VALUE)))),
    }
}

/// Whether an address may be handed to the browser as an image source.
///
/// Only absolute `http`/`https`. A relative address cannot be resolved without knowing the
/// indexer's base, and every other scheme — `data:` above all — has no business in an
/// `<img src>` built from a third party's response.
fn is_displayable_image(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// Inserts while the shared budget lasts, so the total stays bounded whatever the mix.
fn insert_within(
    attributes: &mut BTreeMap<String, String>,
    budget: &mut usize,
    name: String,
    value: String,
) {
    let cost = name.len() + value.len();
    if cost > *budget {
        return;
    }
    *budget -= cost;
    attributes.insert(name, value);
}

/// Shortens to at most `max` bytes without splitting a character.
fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::{MAX_FIELDS, MAX_TOTAL, MAX_VALUE, retain};
    use std::collections::BTreeMap;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn ordinary_attributes_survive_unchanged() {
        let kept = retain(
            &map(&[
                ("imdbscore", "7.8"),
                ("resolution", "1080p"),
                ("grabs", "12"),
            ]),
            None,
        );
        assert_eq!(
            kept.attributes.get("imdbscore").map(String::as_str),
            Some("7.8")
        );
        assert_eq!(
            kept.attributes.get("resolution").map(String::as_str),
            Some("1080p")
        );
        assert_eq!(kept.attributes.get("grabs").map(String::as_str), Some("12"));
        assert!(kept.password.is_none());
    }

    #[test]
    fn attribute_names_are_lowercased_and_trimmed() {
        let kept = retain(
            &map(&[(" CoverUrl ", "https://indexer.example/c.jpg")]),
            None,
        );
        assert!(kept.attributes.contains_key("coverurl"));
    }

    #[test]
    fn credential_bearing_names_are_dropped_whole() {
        let kept = retain(
            &map(&[
                ("apikey", "deadbeef"),
                ("passkey", "cafebabe"),
                ("token", "sekrit"),
                ("title", "Some.Release"),
            ]),
            None,
        );
        assert_eq!(kept.attributes.len(), 1);
        assert!(kept.attributes.contains_key("title"));
    }

    #[test]
    fn an_api_key_inside_a_value_is_redacted() {
        // Torznab indexers put the passkey in the magnet or the nfo address.
        let kept = retain(
            &map(&[(
                "nfo",
                "https://indexer.example/nfo?apikey=deadbeefcafe&id=7",
            )]),
            None,
        );
        let nfo = kept.attributes.get("nfo").expect("nfo kept");
        assert!(!nfo.contains("deadbeefcafe"), "{nfo}");
        // Structure survives redaction, so the value is still readable.
        assert!(nfo.contains("apikey="), "{nfo}");
        assert!(nfo.contains("id=7"), "{nfo}");
    }

    #[test]
    fn cover_addresses_must_be_absolute_http() {
        for bad in [
            "data:image/png;base64,AAAA",
            "javascript:alert(1)",
            "/covers/12345.jpg",
            "not a url at all",
        ] {
            let kept = retain(&map(&[("coverurl", bad)]), None);
            assert!(
                kept.attributes.is_empty(),
                "kept {bad}: {:?}",
                kept.attributes
            );
        }
        let good = retain(&map(&[("coverurl", "https://indexer.example/c.jpg")]), None);
        assert_eq!(
            good.attributes.get("coverurl").map(String::as_str),
            Some("https://indexer.example/c.jpg")
        );
    }

    #[test]
    fn the_password_flag_is_a_flag_and_never_a_password() {
        // The specification's values: 0 no, 1 rar pass, 2 inner archive.
        let none = retain(&map(&[("password", "0")]), None);
        assert!(none.attributes.is_empty(), "a zero flag says nothing");
        assert!(none.password.is_none());

        for flag in ["1", "2"] {
            let kept = retain(&map(&[("password", flag)]), None);
            assert_eq!(
                kept.attributes.get("password").map(String::as_str),
                Some(flag)
            );
            assert!(
                kept.password.is_none(),
                "the flag must never be stored as a password"
            );
        }
    }

    #[test]
    fn a_non_numeric_password_is_taken_as_the_secret() {
        let kept = retain(&map(&[("password", "hunter2")]), None);
        assert_eq!(kept.password.as_deref(), Some("hunter2"));
        // The flag the client sees says "protected" without disclosing the secret.
        assert_eq!(
            kept.attributes.get("password").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn an_empty_password_attribute_is_ignored() {
        let kept = retain(&map(&[("password", "   ")]), None);
        assert!(kept.attributes.is_empty());
        assert!(kept.password.is_none());
    }

    #[test]
    fn the_enclosure_size_fills_in_only_when_the_attribute_is_missing() {
        let from_enclosure = retain(&map(&[]), Some(4_200));
        assert_eq!(
            from_enclosure.attributes.get("size").map(String::as_str),
            Some("4200")
        );

        let attribute_wins = retain(&map(&[("size", "999")]), Some(4_200));
        assert_eq!(
            attribute_wins.attributes.get("size").map(String::as_str),
            Some("999")
        );

        // A zero-length enclosure is an absent size, not a size of zero.
        assert!(retain(&map(&[]), Some(0)).attributes.is_empty());
    }

    #[test]
    fn long_values_are_truncated_on_a_character_boundary() {
        // A three-byte character, so the cut lands mid-sequence unless it is guarded.
        let plot = "\u{4e2d}".repeat(MAX_VALUE);
        let kept = retain(&map(&[("imdbplot", &plot)]), None);
        let stored = kept.attributes.get("imdbplot").expect("plot kept");
        assert!(stored.len() <= MAX_VALUE);
        assert!(!stored.is_empty());
    }

    #[test]
    fn the_field_count_is_capped() {
        let pairs: Vec<(String, String)> = (0..MAX_FIELDS * 2)
            .map(|index| (format!("attr{index:03}"), "x".to_owned()))
            .collect();
        let raw: BTreeMap<String, String> = pairs.into_iter().collect();
        assert_eq!(retain(&raw, None).attributes.len(), MAX_FIELDS);
    }

    #[test]
    fn the_total_size_is_capped() {
        let raw: BTreeMap<String, String> = (0..MAX_FIELDS)
            .map(|index| (format!("attr{index:03}"), "y".repeat(MAX_VALUE)))
            .collect();
        let kept = retain(&raw, None);
        let total: usize = kept
            .attributes
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum();
        assert!(total <= MAX_TOTAL, "{total} over budget");
        assert!(
            !kept.attributes.is_empty(),
            "the budget must still admit some"
        );
    }

    #[test]
    fn empty_values_are_not_stored() {
        let kept = retain(&map(&[("genre", "   "), ("year", "")]), None);
        assert!(kept.attributes.is_empty());
    }
}
