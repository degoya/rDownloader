//! The file name a `Content-Disposition` header names (RFC 6266), once (RD-191-07, PLUG-10).
//!
//! Eight plugins split the header at every `;` and took the first `filename=`. That cut a
//! quoted name at a `;` inside it (`"Part 1; Part 2.rar"` became `"Part 1`), and it never read
//! `filename*=`, the parameter RFC 6266 puts a non-ASCII name in — so a hoster that sends both
//! got its ASCII stand-in used, and one that sends only the extended form got no name at all.
//!
//! What the name is used for is the host's business: it sanitises every file name a plugin
//! reports before one reaches the disk. This only reads the header faithfully. Plain Rust with
//! no dependencies, so a guest that takes it gains no import.

use crate::encode::percent_decode;

/// The file name a `Content-Disposition` value names, or `None` when it names none.
///
/// `filename*=` wins over `filename=` when both are there and the extended one decodes, as RFC
/// 6266 section 4.3 asks; an extended value in a charset other than UTF-8 or ISO-8859-1 is skipped
/// rather than guessed at. A quoted value keeps a `;` inside it and loses its backslash escapes;
/// an unquoted one runs to the next `;`. A value wrapped in single quotes — not in the RFC, but
/// what some hosters send — loses them, as it always did here.
#[must_use]
pub fn file_name_from_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    let mut extended = None;
    for (name, value) in parameters(value) {
        if name.eq_ignore_ascii_case("filename*") {
            if extended.is_none() {
                extended = decode_extended(&value);
            }
        } else if name.eq_ignore_ascii_case("filename") && plain.is_none() {
            plain = Some(value);
        }
    }
    extended
        .or(plain)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// The `name=value` parameters after the disposition type, values unquoted.
fn parameters(header: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = header.chars().peekable();
    // The disposition type (`attachment`, `inline`) comes first and is no parameter; a header
    // that starts straight with a parameter — which some hosters send — is read all the same.
    let mut segment = String::new();
    loop {
        // One parameter name, up to `=` or `;`.
        segment.clear();
        while let Some(&character) = chars.peek() {
            if character == '=' || character == ';' {
                break;
            }
            segment.push(character);
            chars.next();
        }
        let name = segment.trim().to_owned();
        match chars.next() {
            None => break,
            Some(';') => continue,
            Some(_) => {}
        }
        // The value: a quoted string, or everything up to the next `;`.
        while chars
            .peek()
            .is_some_and(|character| *character == ' ' || *character == '\t')
        {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(character) = chars.next() {
                match character {
                    '"' => break,
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    other => value.push(other),
                }
            }
            // Whatever follows the closing quote up to the next `;` is not part of the value.
            for character in chars.by_ref() {
                if character == ';' {
                    break;
                }
            }
        } else {
            for character in chars.by_ref() {
                if character == ';' {
                    break;
                }
                value.push(character);
            }
            let trimmed = value.trim();
            value = trimmed
                .strip_prefix('\'')
                .and_then(|inner| inner.strip_suffix('\''))
                .unwrap_or(trimmed)
                .to_owned();
        }
        if !name.is_empty() {
            out.push((name, value));
        }
    }
    out
}

/// An RFC 8187 `ext-value` — `charset'language'percent-encoded` — decoded, when its charset is
/// one this can decode.
fn decode_extended(value: &str) -> Option<String> {
    let mut parts = value.trim().splitn(3, '\'');
    let charset = parts.next()?;
    let _language = parts.next()?;
    let encoded = parts.next()?;
    let bytes = percent_decode(encoded);
    if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.into_iter().map(char::from).collect())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::file_name_from_disposition as name;

    #[test]
    fn a_plain_name_is_read_quoted_and_unquoted() {
        assert_eq!(
            name(r#"attachment; filename="report.pdf""#).as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            name("attachment; filename=report.pdf").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            name("attachment;filename=report.pdf;size=10").as_deref(),
            Some("report.pdf")
        );
        assert_eq!(
            name("Attachment; FILENAME='legacy.zip'").as_deref(),
            Some("legacy.zip")
        );
        // Some hosters send the parameter without a type in front of it.
        assert_eq!(name("filename=bare.bin").as_deref(), Some("bare.bin"));
    }

    /// The bug the eight copies shared: a `;` inside quotes ended the name.
    #[test]
    fn a_semicolon_inside_quotes_is_part_of_the_name() {
        assert_eq!(
            name(r#"attachment; filename="Part 1; Part 2.rar"; size=3"#).as_deref(),
            Some("Part 1; Part 2.rar")
        );
        assert_eq!(
            name(r#"attachment; filename="say \"hi\".txt""#).as_deref(),
            Some(r#"say "hi".txt"#)
        );
    }

    /// The other bug: `filename*=` was never read, so a non-ASCII name was lost.
    #[test]
    fn the_extended_name_wins_and_is_decoded() {
        assert_eq!(
            name("attachment; filename=\"Uebersicht.pdf\"; filename*=UTF-8''%C3%9Cbersicht.pdf")
                .as_deref(),
            Some("\u{dc}bersicht.pdf")
        );
        assert_eq!(
            name("attachment; filename*=utf-8'de'Gr%C3%BC%C3%9Fe%3B%20alle.txt").as_deref(),
            Some("Gr\u{fc}\u{df}e; alle.txt")
        );
        assert_eq!(
            name("attachment; filename*=ISO-8859-1''caf%E9.txt").as_deref(),
            Some("caf\u{e9}.txt")
        );
    }

    /// An extended value that cannot be decoded falls back to the plain one rather than
    /// producing a guess.
    #[test]
    fn an_undecodable_extended_name_falls_back() {
        assert_eq!(
            name("attachment; filename*=KOI8-R''%E1; filename=fallback.txt").as_deref(),
            Some("fallback.txt")
        );
        assert_eq!(
            name("attachment; filename*=UTF-8''%FF%FE; filename=\"plain.txt\"").as_deref(),
            Some("plain.txt")
        );
    }

    #[test]
    fn a_header_without_a_name_has_none() {
        assert_eq!(name("attachment"), None);
        assert_eq!(name("inline; size=12"), None);
        assert_eq!(name(r#"attachment; filename="""#), None);
        assert_eq!(name("attachment; filename=   "), None);
        assert_eq!(name(""), None);
        // A `filenamex=` is a different parameter.
        assert_eq!(name("attachment; filenamex=a.txt"), None);
    }
}
