//! The small amount of JSON reading a sign-in exchange needs, without a parser (RD-191-07,
//! PLUG-08).
//!
//! A token or device-code answer is a flat document of a handful of fields, and pulling a JSON
//! library into a sandboxed guest to read five strings would be more code and more surface for
//! no more capability. Five sign-in plugins carried their own copy of these two readers;
//! a correction to one now reaches all of them. A plugin that reads a nested document — or one
//! a stranger shapes, such as a provider's account data — uses `serde_json` instead.

/// The string value of a JSON field, unescaped.
///
/// Reads out of the original text rather than a whitespace-stripped copy: the spaces inside a
/// value are part of it, and squeezing them out turns "The PIN has expired" into something
/// nobody wrote. `\uXXXX` escapes are decoded, a surrogate pair (an emoji, say) into the one
/// character it spells and a lone surrogate into U+FFFD, as `serde_json` reads them lossily;
/// an escape that is not four hex digits ends the read with `None` rather than inventing a
/// character.
#[must_use]
pub fn string_field(body: &str, name: &str) -> Option<String> {
    let mut chars = value_after(body, name)?.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let unit = hex_unit(&mut chars)?;
                    out.push(match unit {
                        0xD800..=0xDBFF => {
                            // A high surrogate means something only with the low one after it;
                            // that one is looked at without being taken, so a lone high
                            // surrogate leaves whatever follows it to be read as itself.
                            let mut ahead = chars.clone();
                            let low = (ahead.next() == Some('\\') && ahead.next() == Some('u'))
                                .then(|| hex_unit(&mut ahead))
                                .flatten()
                                .filter(|low| (0xDC00..=0xDFFF).contains(low));
                            match low {
                                Some(low) => {
                                    chars = ahead;
                                    char::from_u32(
                                        0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00),
                                    )
                                    .unwrap_or(char::REPLACEMENT_CHARACTER)
                                }
                                None => char::REPLACEMENT_CHARACTER,
                            }
                        }
                        0xDC00..=0xDFFF => char::REPLACEMENT_CHARACTER,
                        unit => char::from_u32(unit)?,
                    });
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

/// Four hex digits of a `\u` escape, or `None` when they are not.
fn hex_unit(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    let digits: String = chars.by_ref().take(4).collect();
    if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(&digits, 16).ok()
}

/// The non-negative integer value of a JSON field. A quoted number is not one.
#[must_use]
pub fn number_field(body: &str, name: &str) -> Option<u64> {
    let rest = value_after(body, name)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The text just after `"name":`, with the separating whitespace skipped.
///
/// The first occurrence of the name *as a key* wins: a `"name"` that is not followed by a
/// colon is a value somebody wrote, and the search goes on past it.
fn value_after<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("\"{name}\"");
    let mut from = 0;
    while let Some(at) = body[from..].find(&needle) {
        let after = &body[from + at + needle.len()..];
        if let Some(value) = after.trim_start().strip_prefix(':') {
            return Some(value.trim_start());
        }
        from += at + needle.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{number_field, string_field};

    #[test]
    fn json_fields_are_read_without_a_parser() {
        let body = r#"{"access_token":"a b","expires_in":3600,"error":"invalid_grant"}"#;
        assert_eq!(string_field(body, "access_token").as_deref(), Some("a b"));
        assert_eq!(number_field(body, "expires_in"), Some(3600));
        assert_eq!(string_field(body, "refresh_token"), None);
    }

    #[test]
    fn fields_are_read_out_of_pretty_printed_and_compact_documents_alike() {
        assert_eq!(string_field(r#"{"a":"b"}"#, "a").as_deref(), Some("b"));
        assert_eq!(
            string_field("{\n  \"a\" : \"b\"\n}", "a").as_deref(),
            Some("b")
        );
        assert_eq!(number_field(r#"{"n": 42, "m": 1}"#, "n"), Some(42));
        assert_eq!(number_field(r#"{"n":"42"}"#, "n"), None);
    }

    #[test]
    fn escapes_are_decoded() {
        assert_eq!(
            string_field(r#"{"u":"https:\/\/x.test\/a"}"#, "u").as_deref(),
            Some("https://x.test/a")
        );
        assert_eq!(
            string_field(r#"{"q":"say \"hi\"\n"}"#, "q").as_deref(),
            Some("say \"hi\"\n")
        );
        assert_eq!(
            string_field(r#"{"e":"caf\u00e9"}"#, "e").as_deref(),
            Some("caf\u{e9}")
        );
        assert_eq!(string_field(r#"{"e":"\uZZZZ"}"#, "e"), None);
        assert_eq!(string_field(r#"{"e":"open"#, "e"), None);
    }

    /// A character outside the basic plane arrives as a surrogate pair and is read as the one
    /// character it is; a surrogate on its own is U+FFFD, and what follows it is kept
    /// (RA-PLG-07).
    #[test]
    fn surrogate_pairs_are_one_character_and_a_lone_half_is_replaced() {
        assert_eq!(
            string_field(r#"{"n":"ok \ud83d\ude00!"}"#, "n").as_deref(),
            Some("ok \u{1F600}!")
        );
        assert_eq!(
            string_field(r#"{"n":"\uD83D\uDE00"}"#, "n").as_deref(),
            Some("\u{1F600}")
        );
        assert_eq!(
            string_field(r#"{"n":"a\ud83db"}"#, "n").as_deref(),
            Some("a\u{FFFD}b")
        );
        assert_eq!(
            string_field(r#"{"n":"a\ude00b"}"#, "n").as_deref(),
            Some("a\u{FFFD}b")
        );
        // A high surrogate followed by an escape that is not a low one keeps that escape.
        assert_eq!(
            string_field(r#"{"n":"\ud83d\u00e9"}"#, "n").as_deref(),
            Some("\u{FFFD}\u{e9}")
        );
        assert_eq!(
            string_field(r#"{"n":"\ud83d\n"}"#, "n").as_deref(),
            Some("\u{FFFD}\n")
        );
        assert_eq!(
            string_field(r#"{"n":"\ud83d"}"#, "n").as_deref(),
            Some("\u{FFFD}")
        );
        assert_eq!(string_field(r#"{"n":"\u12"}"#, "n"), None);
    }

    /// A key name that also occurs as a value is skipped until it occurs as a key.
    #[test]
    fn a_name_inside_a_value_is_not_the_key() {
        let body = r#"{"note":"token","token":"t1"}"#;
        assert_eq!(string_field(body, "token").as_deref(), Some("t1"));
    }
}
